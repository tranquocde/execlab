//! Tier A: one row per (combination, session). Always written, tiny.
//!
//! Tier B (fill-level detail, intra-session equity curve) is NOT written during
//! a sweep — it is regenerated on demand by replay, which is legitimate only
//! because the run is deterministic. See `output.rs`.
//!
//! Everything here comes from the bot's FINAL state (`StateValues`,
//! `types.rs:733`). We deliberately do not attach a `BacktestRecorder`: it
//! allocates a `Record` per sample, which is tier-B data we'd throw away.
//! See the "what needs an observer" note at the bottom for the fields that are
//! genuinely unavailable this way.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionRow {
    /// Filename stem — stable id, and the handle tier-B replay takes.
    pub session_id: String,
    pub source_file: String,
    /// Engine timestamp at session end (ns).
    ///
    /// REQUIRED: the UI sorts on this to cumsum a PnL curve. End rather than
    /// start because the bot exposes `current_timestamp()`, and for ordering
    /// sessions the two are equivalent — sessions don't overlap.
    pub session_ts: i64,
    /// Engine timestamp immediately after the strategy's first elapse call.
    #[serde(default)]
    pub session_start_ts: Option<i64>,

    // ---- ledger ----
    /// Total PnL under the configured terminal-valuation mode.
    pub pnl: f64,
    /// Cash at session end, before settling inventory.
    pub balance: f64,
    /// Cumulative fees (negative = rebate earned).
    pub fee: f64,
    /// Inventory held immediately before the strategy starts.
    #[serde(default)]
    pub start_position: f64,
    /// First valid book reference price observed after market data starts.
    #[serde(default)]
    pub arrival_mid_price: Option<f64>,
    /// Leftover inventory at the end of the session.
    pub final_inventory: f64,
    /// Terminal valuation price: binary outcome for prediction markets, or
    /// final reference price for mark-to-market assets.
    pub settle: Option<f64>,
    /// Final book reference price (mid when two-sided, otherwise the available
    /// side), falling back to the settlement value when the final book is empty.
    #[serde(default)]
    pub final_mid_price: Option<f64>,
    /// Direction-aware implementation-shortfall components in quote currency.
    /// Positive is adverse and negative means execution beat arrival.
    #[serde(default)]
    pub filled_cost: Option<f64>,
    #[serde(default)]
    pub filled_cost_pct: Option<f64>,
    #[serde(default)]
    pub residual_cost: Option<f64>,
    #[serde(default)]
    pub residual_cost_pct: Option<f64>,
    #[serde(default)]
    pub implementation_shortfall: Option<f64>,
    /// Percentage of initial notional (`start_position * arrival_mid_price`).
    #[serde(default)]
    pub implementation_shortfall_pct: Option<f64>,
    /// Mean and maximum effective-spread / external-spread across valid samples.
    #[serde(default)]
    pub mean_divergence_score: Option<f64>,
    #[serde(default)]
    pub max_divergence_score: Option<f64>,

    // ---- activity ----
    /// Child-order submission calls made by the strategy.
    #[serde(default)]
    pub num_orders: usize,
    /// Submitted limit orders; this does not assert they filled as makers.
    #[serde(default)]
    pub n_maker: usize,
    pub num_trades: i64,
    pub trading_volume: f64,
    pub trading_value: f64,

    /// `Some(reason)` if the session failed; `pnl` is then meaningless.
    pub error: Option<String>,
}

// NOTE: `extract` (bot -> SessionRow) lives in `sweep/src/extract.rs`, because
// it needs the engine. Keeping `core` free of hftbacktest means the viz binary
// never builds it.

impl SessionRow {
    /// A session that blew up. Keeps the row present so `n_failed` is visible
    /// rather than the session silently vanishing from the sample.
    pub fn failed(file: &Path, reason: String) -> Self {
        Self {
            session_id: file.file_stem().unwrap().to_string_lossy().into_owned(),
            source_file: file.to_string_lossy().into_owned(),
            session_ts: 0,
            session_start_ts: None,
            pnl: f64::NAN,
            balance: f64::NAN,
            fee: f64::NAN,
            start_position: f64::NAN,
            arrival_mid_price: None,
            final_inventory: f64::NAN,
            settle: None,
            final_mid_price: None,
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
            num_trades: 0,
            trading_volume: 0.0,
            trading_value: 0.0,
            error: Some(reason),
        }
    }

    pub fn compute_execution_costs(&mut self) {
        let arrival = self.arrival_mid_price.filter(|price| price.is_finite());
        let final_mid = self.final_mid_price.filter(|price| price.is_finite());
        let direction = (self.start_position.is_finite()
            && self.start_position.abs() > f64::EPSILON)
            .then(|| self.start_position.signum());

        self.filled_cost = arrival
            .zip(direction)
            .filter(|_| self.trading_volume.is_finite() && self.trading_value.is_finite())
            .map(|(price, direction)| {
                direction * (self.trading_volume * price - self.trading_value)
            });
        self.residual_cost = arrival
            .zip(final_mid)
            .filter(|_| self.final_inventory.is_finite())
            .map(|(arrival, final_mid)| self.final_inventory * (arrival - final_mid));
        let initial_notional = arrival
            .filter(|arrival| {
                self.start_position.is_finite()
                    && (self.start_position * arrival).abs() > f64::EPSILON
            })
            .map(|arrival| (self.start_position * arrival).abs());
        self.filled_cost_pct = self
            .filled_cost
            .zip(initial_notional)
            .map(|(cost, notional)| cost / notional * 100.0);
        self.residual_cost_pct = self
            .residual_cost
            .zip(initial_notional)
            .map(|(cost, notional)| cost / notional * 100.0);
        self.implementation_shortfall = self
            .filled_cost
            .zip(self.residual_cost)
            .filter(|_| self.fee.is_finite())
            .map(|(filled, residual)| filled + residual + self.fee);
        self.implementation_shortfall_pct = self
            .implementation_shortfall
            .zip(initial_notional)
            .map(|(shortfall, notional)| shortfall / notional * 100.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn costs(
        start_position: f64,
        final_inventory: f64,
        arrival: f64,
        final_mid: f64,
        trading_volume: f64,
        average_fill: f64,
        fee: f64,
    ) -> SessionRow {
        let mut row = SessionRow::failed(Path::new("session.npz"), String::new());
        row.start_position = start_position;
        row.final_inventory = final_inventory;
        row.arrival_mid_price = Some(arrival);
        row.final_mid_price = Some(final_mid);
        row.trading_volume = trading_volume;
        row.trading_value = trading_volume * average_fill;
        row.fee = fee;
        row.compute_execution_costs();
        row
    }

    #[test]
    fn sell_beating_arrival_is_negative() {
        let row = costs(1_000.0, 0.0, 100.0, 100.0, 1_000.0, 101.0, 0.0);
        assert_eq!(row.filled_cost, Some(-1_000.0));
        assert_eq!(row.implementation_shortfall, Some(-1_000.0));
        assert_eq!(row.implementation_shortfall_pct, Some(-1.0));
    }

    #[test]
    fn buy_beating_arrival_is_negative() {
        let row = costs(-1_000.0, 0.0, 100.0, 100.0, 1_000.0, 99.0, 0.0);
        assert_eq!(row.filled_cost, Some(-1_000.0));
        assert_eq!(row.implementation_shortfall, Some(-1_000.0));
        assert_eq!(row.implementation_shortfall_pct, Some(-1.0));
    }

    #[test]
    fn unfinished_buy_before_price_rise_is_positive() {
        let row = costs(-1_000.0, -400.0, 100.0, 110.0, 600.0, 100.0, 0.0);
        assert_eq!(row.filled_cost, Some(0.0));
        assert_eq!(row.residual_cost, Some(4_000.0));
        assert_eq!(row.implementation_shortfall, Some(4_000.0));
        assert_eq!(row.implementation_shortfall_pct, Some(4.0));
    }

    #[test]
    fn unfinished_sell_before_price_fall_is_positive() {
        let row = costs(1_000.0, 400.0, 100.0, 90.0, 600.0, 100.0, 0.0);
        assert_eq!(row.residual_cost, Some(4_000.0));
        assert_eq!(row.implementation_shortfall_pct, Some(4.0));
    }
}

// ---------------------------------------------------------------------------
// NOT AVAILABLE from final state — these need an observer.
//
//   max_inventory        peak |position| during the session
//   n_maker / n_taker    StateValues has only `num_trades`, no side breakdown
//   intra-session shape  bled steadily vs one bad settlement — the aggregate
//                        stats cannot tell these apart, and they mean very
//                        different things
//   last two-sided mid   for a robust `settle` (see the soft spot above)
//
// The clean way to get them costs the alphas nothing, and reuses the decision
// we already made: `Alpha::run` is generic over `Bot<MD>`. So we can pass a
// WRAPPER that implements `Bot<MD>`, delegates every call to the real
// `Backtest`, and samples state on each `elapse()`:
//
//     struct Observed<B> { inner: B, max_inv: f64, last_two_sided_mid: f64 }
//     impl<MD, B: Bot<MD>> Bot<MD> for Observed<B> { ... }
//
// The alpha never knows. This is the same property that makes live deployment a
// swap rather than a rewrite — worth building before trusting `max_inventory`
// or any risk metric.
//
// Adverse selection (did the mid move against us right after each fill?) needs
// fill-level data, so it belongs in tier B, computed at replay.
// ---------------------------------------------------------------------------

/// Every row produced so far, keyed by param hash.
///
/// Dead weight under `Grid`; the entire input to a GA's fitness function.
/// Carried now precisely so adding the GA needs no restructuring.
#[derive(Default)]
pub struct Results {
    pub by_hash: std::collections::HashMap<String, Vec<SessionRow>>,
}

impl Results {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn absorb(&mut self, hash: &str, rows: Vec<SessionRow>) {
        self.by_hash.insert(hash.to_string(), rows);
    }
}
