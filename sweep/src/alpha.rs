//! The contract every alpha implements. Written once, never touched again.

use execlab_core::ExecutionTarget;
use hftbacktest::prelude::{Bot, MarketDepth};
use serde::{de::DeserializeOwned, Serialize};

pub trait Alpha {
    /// Everything that varies across the search space. Typed, serializable.
    ///
    /// Use enum fields for STRUCTURAL choices (which exit rule, which quoting
    /// scheme), not just numeric knobs — illegal combinations then fail to
    /// compile, and `search_space()` cross-products over structure for free.
    type Params: Clone + Serialize + DeserializeOwned + Send + Sync + 'static;

    /// The alpha owns its own search space.
    fn search_space() -> Vec<Self::Params>;

    /// Optional execution-target metadata used by generic output accounting.
    fn execution_target(_p: &Self::Params) -> Option<ExecutionTarget> {
        None
    }

    fn has_data_dependent_initial_position() -> bool {
        false
    }

    /// Resolve a data-dependent initial position before the real simulation.
    /// The default keeps the position supplied by `BacktestConfig`.
    fn resolve_initial_position<MD, B>(_hbt: &mut B, _p: &Self::Params) -> Option<f64>
    where
        MD: MarketDepth,
        B: Bot<MD>,
        B::Error: std::fmt::Debug,
    {
        None
    }

    /// The trading logic.
    ///
    /// Generic over `Bot` on purpose: the same source compiles against the
    /// backtest today and against a live bot later, with no rewrite. Never
    /// take a concrete `Backtest<..>` here.
    ///
    /// `Bot` is parameterised by its market-depth type, so both appear here.
    /// Callers never name `MD` — it's inferred from the bot they pass.
    ///
    /// `B::Error: Debug` is required once here so every alpha can `.unwrap()`
    /// engine calls without repeating the bound.
    fn run<MD, B>(hbt: &mut B, p: &Self::Params)
    where
        MD: MarketDepth,
        B: Bot<MD>,
        B::Error: std::fmt::Debug;
}

// NOTE: deliberately no `fn name()`. The FILENAME is the alpha's identity —
// CLI argument, output directory, registry key. One source of truth; build.rs
// supplies it.
