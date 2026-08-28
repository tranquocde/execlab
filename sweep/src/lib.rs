//! execlab — shared library behind the `execlab` (sweep) and `exec_replay`
//! binaries. Binaries cannot import each other, so anything both need lives
//! here.
//!
//! Dependency direction is one-way: `replay` reads `alpha`/`engine`/`extract`;
//! nothing in the sweep path knows `replay` exists.

pub mod alpha;
pub mod alphas;
pub mod engine;
pub mod extract;
pub mod population;
pub mod progress;
pub mod replay;
pub mod status;
pub mod sweep;
pub mod sweep_observer;
pub mod template;

// ---------------------------------------------------------------------------
// The alpha registry.
//
// `build.rs` generates `alphas::registry()` calling `crate::entry::<A>(..)`, so
// these must live in the LIB, not in a binary — both binaries consume the same
// registry. Alphas have different `Params` types, so the closures are boxed to
// erase them.
// ---------------------------------------------------------------------------

use std::path::{Path, PathBuf};

pub type SweepFn = Box<
    dyn Fn(
        &[PathBuf],
        &str,
        &Path,
        &Path,
        usize,
        bool,
        &engine::BacktestConfig,
        Option<&serde_json::Value>,
        Option<&Path>,
    ) -> Result<(), String>,
>;
pub type ReplayFn = Box<dyn Fn(replay::Args) -> Result<replay::ReplayResult, String>>;

pub struct Entry {
    pub name: &'static str,
    pub code_hash: &'static str,
    /// Combinations in the current search space — the `--status` denominator.
    pub total_space: usize,
    pub config_json: &'static str,
    pub sweep: SweepFn,
    pub replay: ReplayFn,
}

pub fn entry<A: alpha::Alpha + 'static>(
    name: &'static str,
    code_hash: &'static str,
    config_json: &'static str,
) -> Entry {
    Entry {
        name,
        code_hash,
        total_space: A::search_space().len(),
        config_json,
        sweep: Box::new(
            move |files, data_dir, data_root, out, batch, force, config, override_params, progress_path| {
                let space = match override_params {
                    Some(value) => {
                        let params = value.get("params").unwrap_or(value).clone();
                        let space: Vec<A::Params> =
                            serde_json::from_value(params).map_err(|error| {
                                format!("invalid parameter override for {name}: {error}")
                            })?;
                        if space.is_empty() {
                            return Err(format!("parameter override for {name} is empty"));
                        }
                        space
                    }
                    None => A::search_space(),
                };
                sweep::sweep::<A, _>(
                    name,
                    code_hash,
                    files,
                    data_dir,
                    data_root,
                    out,
                    population::Grid::new(space, batch), // <- later: Ga::new(..)
                    force,
                    config,
                    progress_path,
                );
                Ok(())
            },
        ),
        replay: Box::new(|args| replay::replay::<A>(args)),
    }
}
