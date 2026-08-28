//! Tier B: re-run ONE session of ONE combination and record what happened.
//!
//! Legitimate only because the run is deterministic — which is why every replay
//! re-verifies its own final PnL against the tier-A row already on disk. If
//! that ever disagrees, caching / resume / gc are all unsafe and you want to
//! know immediately, not six months later.
//!
//! Nothing in the sweep path calls into here; the dependency is one-way.

pub mod observer;
pub mod record;

use std::{
    fs,
    path::{Path, PathBuf},
};

use execlab_core::{Manifest, SessionRow};
use hftbacktest::prelude::Bot;

use crate::{alpha::Alpha, engine, extract::extract};
use observer::Observed;
use record::{ReplayFile, Verify};

/// Cap on curve points written. At `elapse_ns = 1e7` a 5-minute session takes
/// ~30k samples; thinning keeps the file readable. Fills are NEVER thinned.
const MAX_CURVE: usize = 3000;

pub struct Args<'a> {
    pub out_root: &'a Path,
    pub data_root: &'a Path,
    pub alpha: &'a str,
    pub hash: &'a str,
    pub session: &'a str,
    pub code_hash: &'a str,
    pub no_cache: bool,
}

pub struct ReplayResult {
    pub path: PathBuf,
    pub cached: bool,
}

pub fn replay<A: Alpha>(args: Args) -> Result<ReplayResult, String> {
    let dir = args.out_root.join(args.alpha).join(args.hash);

    let manifest: Manifest = fs::read_to_string(dir.join("manifest.json"))
        .map_err(|e| format!("no manifest at {}: {e}", dir.display()))
        .and_then(|t| serde_json::from_str(&t).map_err(|e| format!("bad manifest: {e}")))?;

    // A replay under different logic is not a replay. Hard error.
    if manifest.code_hash != args.code_hash {
        return Err(format!(
            "code_hash mismatch: manifest {}, binary {}\n\
             the alpha, engine.rs or extract.rs changed since these results were produced",
            manifest.code_hash, args.code_hash
        ));
    }

    // Params come back from the manifest — this is what the DeserializeOwned
    // bound on Alpha::Params is for.
    let params: A::Params = serde_json::from_value(manifest.params.clone())
        .map_err(|e| format!("cannot rebuild params from manifest: {e}"))?;
    let backtest_config: engine::BacktestConfig =
        serde_json::from_value(manifest.backtest_config.clone())
            .map_err(|e| format!("cannot rebuild backtest config from manifest: {e}"))?;

    let (market, row) = find_session(&dir, args.session)?;

    let cache = dir
        .join("replay")
        .join(&market)
        .join(format!("{}.json", row.session_id));
    if !args.no_cache && cache.exists() {
        return Ok(ReplayResult {
            path: cache,
            cached: true,
        });
    }

    // The source path comes from the ROW, not rebuilt from the data dir, so it
    // keeps working if the data moved.
    let source = PathBuf::from(&row.source_file);
    let src = if source.is_absolute() {
        // Backward compatibility for outputs created before portable paths.
        source
    } else {
        args.data_root.join(source)
    };
    if !src.exists() {
        return Err(format!("session data missing: {}", src.display()));
    }

    let data = engine::load_session(&src);
    let backtest = engine::build_backtest(data, &backtest_config);
    let initial_position = backtest.position(0);
    let mut hbt = Observed::new(backtest);

    A::run(&mut hbt, &params);

    // Recompute exactly as the sweep does, then check it against tier A.
    // Only numeric fields are compared below; the replay row's source path is
    // not persisted, so use its immediate parent as a harmless local root.
    let replayed = extract(
        &src,
        src.parent().unwrap_or(args.data_root),
        &hbt,
        &backtest_config,
        initial_position,
        hbt.arrival_mid_price,
    );
    let diff = (replayed.pnl - row.pnl).abs();

    // Fill-level self-check: the fills we recorded must account for the whole
    // position the engine ended with.
    let net: f64 = hbt
        .fills
        .iter()
        .map(|f| if f.side == "buy" { f.qty } else { -f.qty })
        .sum();
    let final_position = replayed.final_inventory;

    let verify = Verify {
        tier_a_pnl: row.pnl,
        replay_pnl: replayed.pnl,
        matches: diff <= 1e-9,
        abs_diff: diff,
        initial_position,
        fills_net_qty: net,
        final_position,
        fills_reconcile: (initial_position + net - final_position).abs() <= 1e-9,
    };

    let n_maker = hbt.fills.iter().filter(|f| f.maker).count();
    let samples_taken = hbt.curve.len();
    let curve = thin(hbt.curve, MAX_CURVE);

    let out = ReplayFile {
        alpha: args.alpha.to_string(),
        hash: args.hash.to_string(),
        code_hash: manifest.code_hash.clone(),
        market: market.clone(),
        session_id: args.session.to_string(),
        source_file: row.source_file.clone(),
        max_inventory: hbt.max_inventory,
        n_maker,
        n_taker: hbt.fills.len() - n_maker,
        samples_taken,
        verify: verify.clone(),
        fills: hbt.fills,
        curve,
    };

    write_atomic(&cache, &serde_json::to_vec_pretty(&out).unwrap());
    if !verify.fills_reconcile {
        return Err(format!(
            "FILLS DO NOT RECONCILE: initial position {:.6} + recorded fills {:.6} != \
            final position {:.6}. The observer missed executions — fill-level analysis \
            of this session is unreliable. Written anyway to {}",
            verify.initial_position,
            verify.fills_net_qty,
            verify.final_position,
            cache.display()
        ));
    }
    println!(
        "tier-A pnl {}, replay pnl {}, abs_diff {}, final_pos {}",
        verify.tier_a_pnl, verify.replay_pnl, verify.abs_diff, verify.final_position,
    );

    if !verify.matches {
        return Err(format!(
            "NON-DETERMINISTIC: tier-A pnl {:.10} but replay produced {:.10} (diff {:.3e}).\n\
             Cached results, --resume and gc all assume replayability. Written anyway to {}",
            verify.tier_a_pnl,
            verify.replay_pnl,
            verify.abs_diff,
            cache.display()
        ));
    }
    Ok(ReplayResult {
        path: cache,
        cached: false,
    })
}

/// Locate one fully-qualified session: `<timeframe>/<market>/<session-id>`.
fn find_session(dir: &Path, selector: &str) -> Result<(String, SessionRow), String> {
    let parts: Vec<&str> = selector
        .trim_start_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() != 3 || parts.iter().any(|part| matches!(*part, "." | "..")) {
        return Err(format!(
            "session must be <timeframe>/<market>/<id>, got `{selector}`"
        ));
    }

    let (timeframe, market, session_id) = (parts[0], parts[1], parts[2]);
    let rows_path = dir.join(timeframe).join(format!("{market}.json"));
    let rows: Vec<SessionRow> = fs::read_to_string(&rows_path)
        .map_err(|e| format!("cannot read {}: {e}", rows_path.display()))
        .and_then(|text| {
            serde_json::from_str(&text)
                .map_err(|e| format!("cannot parse {}: {e}", rows_path.display()))
        })?;

    let row = rows
        .into_iter()
        .find(|row| row.session_id == session_id)
        .ok_or_else(|| {
            format!(
                "session `{session_id}` not found in {}",
                rows_path.display()
            )
        })?;

    Ok((format!("{timeframe}/{market}"), row))
}

/// Keep every Nth sample, always including the last — so the curve ends where
/// the session actually ended rather than wherever the stride landed.
fn thin<T: Clone>(v: Vec<T>, max: usize) -> Vec<T> {
    if v.len() <= max || v.is_empty() {
        return v;
    }
    let step = v.len().div_ceil(max);
    let mut out: Vec<T> = v.iter().step_by(step).cloned().collect();
    out.push(v.last().unwrap().clone());
    out
}

fn write_atomic(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).unwrap();
    fs::rename(&tmp, path).unwrap();
}
