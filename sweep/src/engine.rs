//! Backtest construction and data loading.
//!
//! The one thing that matters here: `DataSource::Data` instead of
//! `DataSource::File`. `record_only_backtest.rs:61` passes a path, so the file
//! is parsed once per backtest. We parse once per FILE and hand the loaded
//! `Data<Event>` to every param in the batch — `Data` is `{ Rc<DataPtr>, .. }`
//! with a derived `Clone`, so `data.clone()` bumps a refcount and shares the
//! buffer. No copy.
//!
//! `Data` is `Rc`, not `Arc` — it cannot cross threads. That is fine and in
//! fact shapes the design: each worker loads and owns its own session, so peak
//! RAM is `n_threads * one session`, independent of how many sessions exist.

use std::{
    fs,
    path::{Path, PathBuf},
};

use hftbacktest::{
    backtest::{
        assettype::LinearAsset,
        data::{read_npz_file, Data},
        models::{CommonFees, ConstantLatency, RiskAdverseQueueModel, TradingValueFeeModel},
        Backtest, DataSource, ExchangeKind, L2AssetBuilder,
    },
    prelude::HashMapMarketDepth,
    types::Event,
};
use serde::{Deserialize, Serialize};

// Real tick size for these Polymarket binary markets is 0.01.
const TICK_SIZE: f64 = 0.01;
const LOT_SIZE: f64 = 0.001;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum TerminalValuation {
    BinarySettlement,
    FinalMidPrice,
}

impl Default for TerminalValuation {
    fn default() -> Self {
        Self::BinarySettlement
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BacktestConfig {
    pub exchange: String,
    pub queue_model: String,
    pub asset_type: String,
    pub contract_size: f64,
    pub tick_size: f64,
    pub lot_size: f64,
    pub latency_model: String,
    pub entry_latency_ns: i64,
    pub response_latency_ns: i64,
    pub fee_model: String,
    pub maker_fee: f64,
    pub taker_fee: f64,
    pub initial_balance: f64,
    pub initial_position: f64,
    pub last_trades_capacity: usize,
    #[serde(default)]
    pub terminal_valuation: TerminalValuation,
}

/// Single source of truth for HBT construction and output metadata.
impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            exchange: "EffectivePartialFillExchange".into(),
            queue_model: "RiskAdverseQueueModel".into(),
            asset_type: "LinearAsset".into(),
            contract_size: 1.0,
            tick_size: TICK_SIZE,
            lot_size: LOT_SIZE,
            latency_model: "ConstantLatency".into(),
            entry_latency_ns: 10_000_000,
            response_latency_ns: 10_000_000,
            fee_model: "TradingValueFeeModel<CommonFees>".into(),
            maker_fee: 0.005,
            taker_fee: 0.005,
            initial_balance: 0.0,
            initial_position: 100_000.0,
            last_trades_capacity: 1_000,
            terminal_valuation: TerminalValuation::BinarySettlement,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlphaRunConfig {
    pub data_dir: String,
    pub output_dir: String,
    pub backtest_config: BacktestConfig,
}

impl AlphaRunConfig {
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|error| format!("invalid alpha config: {error}"))
    }
}

pub fn backtest_config() -> serde_json::Value {
    serde_json::to_value(BacktestConfig::default()).unwrap()
}

/// Lifted from `record_only_backtest.rs:61`, with `File` swapped for `Data`.
pub fn build_backtest(data: Data<Event>, config: &BacktestConfig) -> Backtest<HashMapMarketDepth> {
    let (tick_size, lot_size) = (config.tick_size, config.lot_size);
    let exchange = match config.exchange.as_str() {
        "PartialFillExchange" => ExchangeKind::PartialFillExchange,
        "NoPartialFillExchange" => ExchangeKind::NoPartialFillExchange,
        "EffectivePartialFillExchange" => ExchangeKind::EffectivePartialFillExchange,
        "EffectiveNoPartialFillExchange" => ExchangeKind::EffectiveNoPartialFillExchange,
        value => panic!("unsupported configured exchange: {value}"),
    };
    Backtest::builder()
        .add_asset(
            L2AssetBuilder::new()
                .data(vec![DataSource::Data(data)])
                .latency_model(ConstantLatency::new(
                    config.entry_latency_ns,
                    config.response_latency_ns,
                ))
                .asset_type(LinearAsset::new(config.contract_size))
                .fee_model(TradingValueFeeModel::new(CommonFees::new(
                    config.maker_fee,
                    config.taker_fee,
                )))
                .queue_model(RiskAdverseQueueModel::new())
                .exchange(exchange)
                .last_trades_capacity(config.last_trades_capacity)
                .initial_balance(config.initial_balance)
                .initial_position(config.initial_position)
                .depth(move || HashMapMarketDepth::new(tick_size, lot_size))
                .build()
                .unwrap(),
        )
        .build()
        .unwrap()
}

/// Parse one session. THE cost to amortize — call once per file, not per param.
///
/// `"data"` is the npz array name the engine's own `File` path uses
/// (`hftbacktest/src/backtest/data/reader.rs:420`).
pub fn load_session(path: &Path) -> Data<Event> {
    read_npz_file::<Event>(path.to_str().unwrap(), "data")
        .unwrap_or_else(|e| panic!("failed to load {}: {e}", path.display()))
}

/// One session file = one market expiry. Sorted, so runs are reproducible.
pub fn npz_files(dir: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {dir}: {e}"))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|x| x == "npz")
                && !p
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        })
        .collect();
    files.sort();
    files
}

/// Resolves `--data-dir` into one or more market directories.
///
/// Accepted layouts:
/// - `data/<timeframe>/<market>`: one market
/// - `data/<timeframe>`: all immediate market children
/// - `data`: all markets under all immediate timeframe children
///
/// Mixing NPZ files and nested datasets at the same level is rejected.
pub fn market_dirs(input: &str) -> Result<Vec<PathBuf>, String> {
    let root = Path::new(input);
    if !root.is_dir() {
        return Err(format!("data directory does not exist: {}", root.display()));
    }

    fn discover(dir: &Path, depth: usize) -> Result<Vec<PathBuf>, String> {
        let direct_files = npz_files(&dir.to_string_lossy());
        let mut children: Vec<PathBuf> = fs::read_dir(dir)
            .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_dir())
            .collect();
        children.sort();

        let mut nested = Vec::new();
        if depth < 2 {
            for child in children {
                nested.extend(discover(&child, depth + 1)?);
            }
        }

        if !direct_files.is_empty() && !nested.is_empty() {
            return Err(format!(
                "ambiguous data layout in {}: found both direct .npz files and nested datasets",
                dir.display()
            ));
        }
        if !direct_files.is_empty() {
            return Ok(vec![dir.to_path_buf()]);
        }
        Ok(nested)
    }

    let markets = discover(root, 0)?;
    if markets.is_empty() {
        Err(format!(
            "no .npz files or market subdirectories in {}",
            root.display()
        ))
    } else {
        Ok(markets)
    }
}

/// Returns the common `data` root for discovered
/// `<data>/<timeframe>/<market>` directories.
pub fn data_root(markets: &[PathBuf]) -> Result<PathBuf, String> {
    let root_for = |market: &Path| {
        market
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                format!(
                    "market directory must have <data>/<timeframe>/<market> structure: {}",
                    market.display()
                )
            })
    };

    let root = root_for(
        markets
            .first()
            .ok_or_else(|| "no market directories discovered".to_string())?,
    )?;
    for market in &markets[1..] {
        if root_for(market)? != root {
            return Err("discovered market directories do not share one data root".into());
        }
    }
    Ok(root)
}

/// Identifies the data universe.
///
/// Recorded in every manifest so we can refuse to compare a combination
/// measured on 100 sessions against one measured on 120. Without this, adding
/// data files silently corrupts every cross-combination comparison and nothing
/// warns you.
pub fn hash_file_list(files: &[PathBuf]) -> String {
    let mut h = blake3::Hasher::new();
    for f in files {
        h.update(f.file_name().unwrap().to_string_lossy().as_bytes());
        let len = fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        h.update(&len.to_le_bytes());
    }
    h.finalize().to_hex()[..12].to_string()
}
