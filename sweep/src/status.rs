//! `--status`: what is on disk for an alpha, without opening 200 manifests by
//! hand.
//!
//! Reads the output tree only — never runs anything, never deletes anything.
//! Safe at any time, including mid-sweep: in-flight combinations have no
//! manifest yet and show up as `partial`.

use std::{collections::BTreeMap, fs, path::Path};

use execlab_core::{Manifest, SessionRow};

struct Combo {
    hash: String,
    code_hash: String,
    params: serde_json::Value,
    /// "5m/market_a" -> session count
    markets: BTreeMap<String, usize>,
    mean_pnl: f64,
    n_failed: usize,
}

pub fn report(root: &Path, alpha: &str, current_code_hash: &str, total_space: usize) {
    let dir = root.join(alpha);
    println!("\n{alpha}  (code {current_code_hash})");

    let Ok(entries) = fs::read_dir(&dir) else {
        println!("  combinations : 0/{total_space} complete  — no output yet");
        return;
    };

    let mut combos = Vec::new();
    let mut partial = 0usize;

    for e in entries.flatten() {
        let d = e.path();
        if !d.is_dir() {
            continue;
        }

        // No readable manifest => never completed: killed mid-write, or a
        // sweep is writing it right now.
        let Some(m) = fs::read_to_string(d.join("manifest.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Manifest>(&t).ok())
        else {
            partial += 1;
            continue;
        };

        let (markets, mean_pnl) = read_rows(&d);
        combos.push(Combo {
            hash: m.hash,
            code_hash: m.code_hash,
            params: m.params,
            markets,
            mean_pnl,
            n_failed: m.n_failed,
        });
    }

    // Anything off the current code hash is about to be gc'd on the next run.
    // Surface it while that is still avoidable.
    let stale = combos
        .iter()
        .filter(|c| c.code_hash != current_code_hash)
        .count();
    let current: Vec<&Combo> = combos
        .iter()
        .filter(|c| c.code_hash == current_code_hash)
        .collect();

    let mut line = format!(
        "  combinations : {}/{} complete",
        current.len(),
        total_space
    );
    if partial > 0 {
        line += &format!(", {partial} partial");
    }
    if stale > 0 {
        line += &format!(", {stale} stale (old code — removed on next run)");
    }
    println!("{line}");

    if current.is_empty() {
        return;
    }

    // Markets should be identical across combinations of one sweep. If they are
    // not, results measured on different universes are sitting side by side and
    // any ranking across them is meaningless.
    let markets = &current[0].markets;
    for (name, n) in markets {
        println!("  market       : {name}  ({n} sessions)");
    }
    if current.iter().any(|c| &c.markets != markets) {
        println!("  !! combinations disagree on markets/session counts — NOT comparable");
    }

    let failed: usize = current.iter().map(|c| c.n_failed).sum();
    if failed > 0 {
        println!("  failed       : {failed} session(s) across all combinations");
    }

    let mut ranked: Vec<&&Combo> = current.iter().filter(|c| c.mean_pnl.is_finite()).collect();
    ranked.sort_by(|a, b| b.mean_pnl.total_cmp(&a.mean_pnl));
    if ranked.is_empty() {
        return;
    }

    println!("  best         :");
    for c in ranked.iter().take(3) {
        println!(
            "    {:+.4} /session   {}   {}",
            c.mean_pnl,
            c.hash,
            compact(&c.params)
        );
    }
    let w = ranked.last().unwrap();
    println!("    {:+.4} /session   {}   (worst)", w.mean_pnl, w.hash);
}

/// Rows live at `<combo>/<tenor>/<market>.json`. Glob rather than assume a
/// name, so this keeps working once several markets share a directory.
fn read_rows(combo: &Path) -> (BTreeMap<String, usize>, f64) {
    let mut markets = BTreeMap::new();
    let mut pnls: Vec<f64> = Vec::new();

    for tenor in fs::read_dir(combo).into_iter().flatten().flatten() {
        let t = tenor.path();
        if !t.is_dir() || t.file_name().is_some_and(|n| n == "replay") {
            continue;
        }
        for f in fs::read_dir(&t).into_iter().flatten().flatten() {
            let p = f.path();
            if p.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let Some(rows) = fs::read_to_string(&p)
                .ok()
                .and_then(|s| serde_json::from_str::<Vec<SessionRow>>(&s).ok())
            else {
                continue;
            };
            let name = format!(
                "{}/{}",
                t.file_name().unwrap().to_string_lossy(),
                p.file_stem().unwrap().to_string_lossy()
            );
            markets.insert(name, rows.len());
            pnls.extend(rows.iter().filter(|r| r.error.is_none()).map(|r| r.pnl));
        }
    }

    let mean = if pnls.is_empty() {
        f64::NAN
    } else {
        pnls.iter().sum::<f64>() / pnls.len() as f64
    };
    (markets, mean)
}

/// `{"spread":2,"elapse_ns":1e7}` -> `spread=2 elapse_ns=10000000`
fn compact(v: &serde_json::Value) -> String {
    match v.as_object() {
        Some(map) => map
            .iter()
            .map(|(k, val)| format!("{k}={}", val.to_string().trim_matches('"')))
            .collect::<Vec<_>>()
            .join(" "),
        None => v.to_string(),
    }
}
