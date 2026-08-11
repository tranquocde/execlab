//! Output layout, hashing, and the completion marker.
//!
//! Layout — a combination directory is SELF-CONTAINED. `strategy.rs` +
//! `manifest.json` + results. Nothing outside it is needed to interpret or
//! deploy it. `index.csv` is pure derived data, rebuildable by scanning
//! manifests, so no unique state ever lives outside a combination.
//!
//!   output/<alpha>/<hash>/
//!       manifest.json                  written LAST => means "complete"
//!       strategy.rs                    params baked in, no sweep, deployable
//!       5m/tcb.json                    one row per session, derived from data dir
//!       replay/                        tier B, on demand, deletable

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::row::SessionRow;

// ---------------------------------------------------------------- hashing

/// Directory name = hash(normalized alpha source + canonical params).
///
/// Canonical JSON matters: `1e7` and `10000000.0` must not produce different
/// directories. `code_hash` comes from build.rs and only moves when the logic
/// moves, so comment edits do not orphan the tree.
pub fn param_hash<P: Serialize>(code_hash: &str, p: &P) -> String {
    let json = serde_json::to_string(&(code_hash, p)).unwrap();
    blake3::hash(json.as_bytes()).to_hex()[..8].to_string()
}

// ---------------------------------------------------------------- manifest

#[derive(Serialize, Deserialize)]
pub struct DataRef {
    pub dir: String,
    pub n_sessions: usize,
    pub file_list_hash: String,
}

#[derive(Serialize, Deserialize)]
pub struct Manifest {
    pub alpha: String,
    pub hash: String,
    /// Params and code kept as SEPARATE fields even though the directory name
    /// fuses them — this is what lets you ask "same params, different code:
    /// did my refactor move the numbers?" without a second directory level.
    pub params: serde_json::Value,
    #[serde(default)]
    pub backtest_config: serde_json::Value,
    pub code_hash: String,
    pub data: DataRef,
    pub engine: String,
    pub created_utc: String,
    pub runtime_sec: f64,
    pub n_failed: usize,
}

/// A combination is complete iff its manifest exists AND was measured on the
/// current data universe.
///
/// Directory existence alone is NOT enough: a process killed mid-write leaves a
/// truncated parquet, and you would skip it forever. Hence manifest-written-last
/// as the marker.
pub fn is_complete(dir: &Path, data_dir: &str, expect_files_hash: &str) -> bool {
    let Ok(txt) = fs::read_to_string(dir.join("manifest.json")) else {
        return false;
    };
    if serde_json::from_str::<Manifest>(&txt).is_err() {
        return false;
    }

    // Completion is per market. This allows one combination directory to hold
    // 5m/tcb.json, 5m/other.json, 15m/tcb.json, etc. independently.
    fs::read_to_string(dir.join(completion_path(data_dir)))
        .is_ok_and(|hash| hash == expect_files_hash)
}

// ---------------------------------------------------------------- writing

/// Maps the input data directory to its result path within a combination.
///
/// `/somewhere/data/5m/tcb` becomes `5m/tcb.json`.
fn result_path(data_dir: &str) -> PathBuf {
    let input = Path::new(data_dir);
    let leaf = input
        .file_name()
        .expect("--data-dir must end with a directory name");

    let mut file = PathBuf::from(leaf);
    file.set_extension("json");

    match input.parent().and_then(Path::file_name) {
        Some(parent) => PathBuf::from(parent).join(file),
        None => file,
    }
}

fn completion_path(data_dir: &str) -> PathBuf {
    let mut path = result_path(data_dir);
    path.set_extension("complete");
    path
}

/// Removes only the artifacts belonging to one market.
///
/// Other markets in the same combination directory are deliberately retained.
pub fn clear_market(dir: &Path, data_dir: &str) {
    let rows = result_path(data_dir);
    let completion = completion_path(data_dir);
    let replay = dir.join("replay").join(rows.with_extension(""));

    let _ = fs::remove_file(dir.join(rows));
    let _ = fs::remove_file(dir.join(completion));
    let _ = fs::remove_dir_all(replay);
}

/// tmp + rename. Atomic on Linux. Without this, a killed process leaves a
/// truncated file that every later read treats as valid.
pub fn write_atomic(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).unwrap();
    fs::rename(&tmp, path).unwrap();
}

/// ORDER IS LOad-BEARING: parquet and strategy.rs first, manifest last.
pub fn write_combination<P: Serialize>(
    dir: &Path,
    alpha: &str,
    code_hash: &str,
    params: &P,
    backtest_config: serde_json::Value,
    rows: &[SessionRow],
    market_dir: &str,
    data: DataRef,
    runtime_sec: f64,
    strategy_src: String,
) {
    write_atomic(&dir.join("strategy.rs"), strategy_src.as_bytes());

    // TODO: real parquet via the arrow/parquet crates. JSON placeholder so the
    // draft is readable end to end.
    let rows_path = result_path(market_dir);
    write_atomic(
        &dir.join(rows_path),
        serde_json::to_vec_pretty(rows).unwrap().as_slice(),
    );
    write_atomic(
        &dir.join(completion_path(market_dir)),
        data.file_list_hash.as_bytes(),
    );

    let n_failed = rows.iter().filter(|r| r.error.is_some()).count();
    let manifest = Manifest {
        alpha: alpha.into(),
        hash: param_hash(code_hash, params),
        params: serde_json::to_value(params).unwrap(),
        backtest_config,
        code_hash: code_hash.into(),
        data,
        engine: env!("CARGO_PKG_VERSION").into(), // TODO: real hftbacktest version
        created_utc: "TODO".into(),
        runtime_sec,
        n_failed,
    };
    write_atomic(
        &dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap().as_slice(),
    );
}

// ---------------------------------------------------------------- gc

/// Removes results that no longer belong to the current version of the alpha.
///
/// Two things go:
///   - directories whose manifest records a DIFFERENT `code_hash` — superseded
///     by an edit to the alpha's logic
///   - directories with NO manifest — a run killed mid-write; unreadable by
///     `is_complete` anyway, so they are pure clutter
///
/// Comment-only edits do not trigger this: `build.rs` hashes normalized source.
///
/// TRADEOFF: this destroys the ability to compare the same params across two
/// versions of the logic ("did my edit help?"). `--keep-old` opts out.
///
/// Scoped strictly to `<root>/<alpha>/<hash>/` — never touches sibling alphas.
pub fn gc_stale(root: &Path, alpha: &str, current_code_hash: &str) -> (usize, usize) {
    let alpha_dir = root.join(alpha);
    let Ok(entries) = fs::read_dir(&alpha_dir) else {
        return (0, 0);
    };

    let (mut superseded, mut partial) = (0usize, 0usize);
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }

        let stale = match fs::read_to_string(dir.join("manifest.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Manifest>(&t).ok())
        {
            // Complete, but produced by different logic.
            Some(m) => {
                let s = m.code_hash != current_code_hash;
                if s {
                    superseded += 1;
                }
                s
            }
            // No readable manifest => never completed.
            None => {
                partial += 1;
                true
            }
        };

        if stale {
            if let Err(e) = fs::remove_dir_all(&dir) {
                eprintln!("  gc: failed to remove {}: {e}", dir.display());
            }
        }
    }

    if superseded + partial > 0 {
        eprintln!(
            "  gc: removed {superseded} superseded (old code) + {partial} partial \
             combination dir(s) under {}",
            alpha_dir.display()
        );
    }
    (superseded, partial)
}

/// Flat, greppable map into the tree so you never open 5000 manifests to find
/// something. Derived — safe to delete and rebuild
pub fn write_index(_root: &Path, _seen: &HashMap<String, Manifest>) {
    // TODO
}

#[cfg(test)]
mod tests {
    use super::result_path;
    use std::path::PathBuf;

    #[test]
    fn result_path_uses_last_two_data_directory_components() {
        assert_eq!(
            result_path("/home/user/execlab/data/5m/tcb"),
            PathBuf::from("5m/tcb.json")
        );
        assert_eq!(
            result_path("data/5m/market_a/"),
            PathBuf::from("5m/market_a.json")
        );
        assert_eq!(result_path("tcb"), PathBuf::from("tcb.json"));
    }
}
