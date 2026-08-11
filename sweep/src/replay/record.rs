//! Tier-B record types: what one replayed session produces.
//!
//! Regenerable by construction, so these are never written during a sweep —
//! only on demand, and safe to delete at any time.

use serde::{Deserialize, Serialize};

/// One execution. Every fill is kept; the curve is what gets thinned.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Fill {
    /// Exchange timestamp (ns) — when the match happened.
    pub ts: i64,
    /// Local timestamp (ns) — when the strategy learned of it. The gap is your
    /// latency, and it is where adverse selection hides.
    pub ts_local: i64,
    pub order_id: u64,
    /// "buy" / "sell"
    pub side: String,
    pub px: f64,
    pub qty: f64,
    /// Maker vs taker, straight from the engine. Tier A cannot provide this —
    /// `StateValues` only carries `num_trades`.
    pub maker: bool,
    /// Position and cash AFTER this fill.
    pub position: f64,
    pub balance: f64,
    pub fee: f64,
}

/// State snapshot, taken once per `elapse` then thinned for output.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Sample {
    pub ts: i64,
    /// External best bid/ask. NaN when that side is empty — serialized as null.
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    /// Latency-safe local effective book. Null when effective mode is disabled
    /// or that side has no liquidity.
    pub effective_bid: Option<f64>,
    pub effective_ask: Option<f64>,
    /// Where OUR resting orders sat. This is what turns a price chart into an
    /// explanation: you watch your quote get lifted and the book walk away.
    pub my_bid: Option<f64>,
    pub my_ask: Option<f64>,
    pub position: f64,
    pub balance: f64,
    pub fee: f64,
    /// balance + position*mid - fee, the engine's own equity formula.
    pub equity: Option<f64>,
}

/// The determinism check. Replay recomputes the session's final PnL; it must
/// equal the tier-A row already on disk.
///
/// This is free (the replay ran anyway) and it is what justifies caching,
/// resume, and gc — all of which assume a combination is reproducible.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Verify {
    pub tier_a_pnl: f64,
    pub replay_pnl: f64,
    pub matches: bool,
    pub abs_diff: f64,

    /// Signed sum of recorded fills. Initial position plus this quantity MUST
    /// equal the engine's final position. If it does not, the observer dropped
    /// fills and every fill-level conclusion drawn from this file is wrong.
    ///
    /// This exact check would have caught the recycled-order-id bug (6 fills
    /// recorded as 1) immediately instead of by eye.
    pub initial_position: f64,
    pub fills_net_qty: f64,
    pub final_position: f64,
    pub fills_reconcile: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReplayFile {
    pub alpha: String,
    pub hash: String,
    pub code_hash: String,
    pub market: String,
    pub session_id: String,
    pub source_file: String,

    /// Recovered here but absent from tier A — final state cannot show a peak.
    pub max_inventory: f64,
    pub n_maker: usize,
    pub n_taker: usize,
    /// Samples taken before thinning, so the output does not misrepresent its
    /// own resolution.
    pub samples_taken: usize,

    pub verify: Verify,
    pub fills: Vec<Fill>,
    pub curve: Vec<Sample>,
}
