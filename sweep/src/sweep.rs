//! The runner. Three loops, in the order that matters:
//!
//!   outer   batches           write barrier => progressive output; later, GA generations
//!   middle  FILES, parallel   flat parallelism; ONE parse per file per batch
//!   inner   params, sequential   amortizes that parse across the whole batch
//!
//! Peak RAM = n_threads * one session, independent of dataset size — because
//! `Data` is `Rc` and never crosses threads, each worker owns exactly one.
//!
//! The loop order is the whole point. Params-outer would re-parse every file
//! once per combination: with a 200-point grid that is 200x the I/O for
//! identical results.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Instant,
};

use rayon::prelude::*;
use hftbacktest::prelude::Bot;

use execlab_core::{
    output::{self, DataRef},
    Results, SessionRow,
};

use crate::{
    alpha::Alpha,
    engine::{backtest_config, build_backtest, hash_file_list, load_session},
    extract::extract,
    population::Population,
    progress::Progress,
    sweep_observer::SweepObserved,
    template,
};

pub fn sweep<A, Pop>(
    name: &str,
    code_hash: &str,
    files: &[PathBuf],
    data_dir: &str,
    data_root: &Path,
    out: &Path,
    mut population: Pop,
    force: bool,
) where
    A: Alpha,
    Pop: Population<A::Params>,
{
    // Edit the alpha's logic => new code_hash => every combination lands in a
    // fresh directory and the old ones become unreachable clutter. Always clear
    // them: results for a version of the logic that no longer exists cannot be
    // reproduced or deployed, so keeping them only invites mixing two versions
    // in one stats table.
    output::gc_stale(out, name, code_hash);

    let files_hash = hash_file_list(files);
    let mut results = Results::empty();
    let (mut ran, mut skipped) = (0usize, 0usize);

    let total_combos = population.total_hint();
    let mut batch_no = 0usize;

    // ---- outer: batches (later: GA generations) ----
    while let Some(batch) = population.next_batch(&results) {
        let t0 = Instant::now();
        batch_no += 1;

        // ---- resume: drop combinations already complete ----
        // Per COMBINATION, not per alpha: a run killed at 60% resumes at 60%.
        let todo: Vec<A::Params> = batch
            .into_iter()
            .filter(|p| {
                let dir = out.join(name).join(output::param_hash(code_hash, p));
                let done = !force && output::is_complete(&dir, data_dir, &files_hash);
                if done {
                    skipped += 1;
                } else {
                    ran += 1;
                }
                !done
            })
            .collect();

        // Fully cached batch => zero file reads. Re-running a finished alpha
        // stats a few manifests and exits.
        if todo.is_empty() {
            continue;
        }

        let rows: Mutex<HashMap<String, Vec<SessionRow>>> = Mutex::new(HashMap::new());

        // Progress ticks per SIMULATION, not per combination: combinations only
        // land at the barrier, so a per-combination bar would sit frozen for
        // the whole batch.
        // `ran + skipped` = combinations complete on disk. Using `ran` alone
        // undercounts after a resume, where most of the work is already done.
        let label = match total_combos {
            Some(t) => format!("  batch {batch_no} ({}/{} combos)", ran + skipped, t),
            None => format!("  batch {batch_no}"),
        };
        let progress = Progress::new(label, files.len() * todo.len());

        // ---- middle: parallel over FILES ----
        files.par_iter().for_each(|file| {
            let data = load_session(file); // parse ONCE

            // ---- inner: this batch's params, on resident data ----
            for p in &todo {
                // `data.clone()` = Rc refcount bump, not a copy of the events.
                let mut hbt = SweepObserved::new(build_backtest(data.clone()));
                let start_position = hbt.position(0);

                A::run(&mut hbt, p);

                let mut row = extract(file, data_root, &hbt);
                row.session_start_ts = hbt.session_start_ts();
                row.start_position = start_position;
                row.arrival_mid_price = hbt.arrival_mid_price();
                row.num_orders = hbt.num_orders();
                row.n_maker = hbt.n_maker();
                row.mean_divergence_score = hbt.mean_divergence_score();
                row.max_divergence_score = hbt.max_divergence_score();
                row.compute_execution_costs();

                rows.lock()
                    .unwrap()
                    .entry(output::param_hash(code_hash, p))
                    .or_default()
                    .push(row);

                progress.inc();
            }
            // `data` dropped here -> memory freed before the next file
        });
        progress.finish();

        // ---- barrier: batch complete, write it out ----
        // Writing cannot happen inside the worker: a worker produces rows for
        // ONE session x ALL params, but a result file needs ONE param x ALL
        // sessions. So we pivot here.
        let mut rows = rows.into_inner().unwrap();
        let runtime = t0.elapsed().as_secs_f64() / todo.len().max(1) as f64;

        for p in &todo {
            let h = output::param_hash(code_hash, p);
            let dir = out.join(name).join(&h);

            // Force is scoped to the current market. A parent data directory
            // invokes this sweep once per discovered market, so its scope
            // naturally follows the level supplied to --data-dir.
            if force {
                output::clear_market(&dir, data_dir);
            }

            let mut r = rows.remove(&h).unwrap_or_default();
            // Chronological, so the UI can cumsum straight into a PnL curve.
            r.sort_by_key(|x| x.session_ts);

            let data = DataRef {
                dir: data_root.to_string_lossy().into_owned(),
                n_sessions: files.len(),
                file_list_hash: files_hash.clone(),
            };
            let strategy = template::render(name, &h, code_hash, p, &data);

            output::write_combination(
                &dir,
                name,
                code_hash,
                p,
                backtest_config(),
                &r,
                data_dir,
                data,
                runtime,
                strategy,
            );

            results.absorb(&h, r); // dead weight today, GA fuel tomorrow
        }

        // Best-so-far: the one number worth watching while it runs.
        if let Some((h, mean)) = results
            .by_hash
            .iter()
            .map(|(h, r)| {
                let ok: Vec<f64> = r
                    .iter()
                    .filter(|x| x.error.is_none())
                    .map(|x| x.pnl)
                    .collect();
                (h.clone(), ok.iter().sum::<f64>() / ok.len().max(1) as f64)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
        {
            let of = total_combos.map_or(String::new(), |t| format!("/{t}"));
            eprintln!(
                "  batch {batch_no} written: {} combos{} done | best mean/session {:+.4} ({})",
                ran + skipped,
                of,
                mean,
                h
            );
        }
    }

    eprintln!("{name}: {ran} run, {skipped} skipped (already complete)");
}
