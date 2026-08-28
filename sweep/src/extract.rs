//! Turning a finished backtest into a tier-A `SessionRow`.
//!
//! Lives in `sweep`, not `core`, because it needs the engine (`Bot`,
//! `MarketDepth`). `core` stays free of hftbacktest so the viz never has to
//! build it.
//!
//! ⚠️ This file is in `RESULT_AFFECTING` (see build.rs): editing it changes
//! every number, so it changes `code_hash` and invalidates the output tree.

use std::{fs, path::Path};

use execlab_core::SessionRow;
use hftbacktest::prelude::{Bot, MarketDepth};
use serde::Deserialize;

use crate::engine::{BacktestConfig, TerminalValuation};

#[derive(Deserialize)]
struct SettlementSidecar {
    settle: f64,
}

fn sidecar_settlement(file: &Path) -> Result<Option<f64>, String> {
    let sidecar = file.with_extension("settle.json");
    let text = match fs::read_to_string(&sidecar) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", sidecar.display())),
    };
    let metadata: SettlementSidecar = serde_json::from_str(&text)
        .map_err(|e| format!("invalid settlement sidecar {}: {e}", sidecar.display()))?;
    if metadata.settle == 0.0 || metadata.settle == 1.0 {
        Ok(Some(metadata.settle))
    } else {
        Err(format!(
            "invalid settlement {} in {}; expected 0 or 1",
            metadata.settle,
            sidecar.display()
        ))
    }
}

/// Extract tier-A stats from a finished backtest.
///
/// Call AFTER `A::run` (which ends with `hbt.close()`), while the bot still
/// holds its final depth and state.
pub fn extract<MD, B>(
    file: &Path,
    data_root: &Path,
    hbt: &B,
    config: &BacktestConfig,
    start_position: f64,
    arrival_mid_price: Option<f64>,
) -> SessionRow
where
    MD: MarketDepth,
    B: Bot<MD>,
{
    let sv = hbt.state_values(0);
    let depth = hbt.depth(0);

    // Ground-truth resolution metadata is written beside the NPZ during data
    // preparation. Fall back to the final book only for legacy files without
    // a sidecar.
    let (bb, ba) = (depth.best_bid(), depth.best_ask());
    let reference_price = match (bb.is_finite(), ba.is_finite()) {
        (true, true) => Some((bb + ba) / 2.0),
        (true, false) => Some(bb),
        (false, true) => Some(ba),
        (false, false) => None,
    };
    let book_settlement = reference_price.map(|price| if price >= 0.5 { 1.0 } else { 0.0 });
    let (settle, settlement_error) = match config.terminal_valuation {
        TerminalValuation::BinarySettlement => match sidecar_settlement(file) {
            Ok(Some(settle)) => (Some(settle), None),
            Ok(None) => (book_settlement, None),
            Err(error) => (None, Some(error)),
        },
        TerminalValuation::FinalMidPrice => (reference_price, None),
    };
    let final_mid_price = reference_price.or(settle);

    // Value leftover inventory under the configured terminal mode and deduct
    // fees. Mark-to-market PnL is the change from initial marked wealth;
    // binary mode retains the legacy terminal-wealth convention.
    let final_wealth = settle.map(|price| sv.balance + sv.position * price - sv.fee);
    let pnl = match config.terminal_valuation {
        TerminalValuation::BinarySettlement => final_wealth,
        TerminalValuation::FinalMidPrice => {
            final_wealth
                .zip(arrival_mid_price)
                .map(|(final_wealth, arrival)| {
                    final_wealth - (config.initial_balance + start_position * arrival)
                })
        }
    }
    .unwrap_or(f64::NAN);
    let error = settlement_error
        .or_else(|| {
            settle.is_none().then(|| {
                format!(
                    "terminal valuation unavailable: final best bid/ask are unavailable{}",
                    if matches!(
                        config.terminal_valuation,
                        TerminalValuation::BinarySettlement
                    ) {
                        format!(
                            " and no {} exists",
                            file.with_extension("settle.json").display()
                        )
                    } else {
                        String::new()
                    }
                )
            })
        })
        .or_else(|| {
            (matches!(config.terminal_valuation, TerminalValuation::FinalMidPrice)
                && arrival_mid_price.is_none())
            .then(|| "mark-to-market PnL unavailable: arrival price is missing".into())
        });

    SessionRow {
        session_id: file.file_stem().unwrap().to_string_lossy().into_owned(),
        source_file: file
            .strip_prefix(data_root)
            .unwrap_or_else(|_| {
                panic!(
                    "session {} is outside data root {}",
                    file.display(),
                    data_root.display()
                )
            })
            .to_string_lossy()
            .into_owned(),
        session_ts: hbt.current_timestamp(),
        session_start_ts: None,

        pnl,
        balance: sv.balance,
        fee: sv.fee,
        start_position,
        arrival_mid_price,
        final_inventory: sv.position,
        settle,
        final_mid_price,
        filled_cost: None,
        filled_cost_pct: None,
        residual_cost: None,
        residual_cost_pct: None,
        implementation_shortfall: None,
        implementation_shortfall_pct: None,
        mean_divergence_score: None,
        max_divergence_score: None,

        num_orders: 0,
        n_maker: 0,
        num_trades: sv.num_trades,
        trading_volume: sv.trading_volume,
        trading_value: sv.trading_value,

        error,
    }
}
