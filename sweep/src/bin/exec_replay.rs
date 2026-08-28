//! exec_replay — re-run one session of one combination and record tier B.
//!
//!   exec_replay mm_spread/d92eb780 --data-dir data --session 5m/tcb/session_0384
//!   exec_replay mm_spread/d92eb780 --data-dir data --session 5m/tcb/session_0384 --no-cache
//!   exec_replay mm_spread/d92eb780 --worst 5        (replay the 5 worst)
//!
//! Costs one session (~0.2s), not one sweep — so it is interactive.
//! Output: <out>/<alpha>/<hash>/replay/<market>/<session>.json

use std::{env, fs, path::Path};

use execlab::{alphas, replay};
use execlab_core::SessionRow;

fn main() {
    let mut args = env::args().skip(1);
    let (mut target, mut session, mut data_dir, mut out_dir) =
        (None, None, None, "output".to_string());
    let (mut no_cache, mut worst) = (false, 0usize);

    while let Some(a) = args.next() {
        match a.as_str() {
            "--session" | "-s" => session = args.next(),
            "--data-dir" => data_dir = args.next(),
            "--out" => out_dir = args.next().unwrap_or(out_dir),
            "--no-cache" => no_cache = true,
            "--worst" => worst = args.next().and_then(|v| v.parse().ok()).unwrap_or(5),
            other => target = Some(other.to_string()),
        }
    }

    let Some(target) = target else {
        eprintln!(
            "usage: exec_replay <alpha>/<hash> \
             --data-dir <root> --session <timeframe>/<market>/<id> \
             [--worst N] [--no-cache]"
        );
        std::process::exit(1);
    };
    let Some((alpha, hash)) = target.split_once('/') else {
        eprintln!("target must be <alpha>/<hash>, got `{target}`");
        std::process::exit(1);
    };

    let registry = alphas::registry();
    let Some(e) = registry.iter().find(|e| e.name == alpha) else {
        eprintln!("unknown alpha: {alpha}");
        std::process::exit(1);
    };

    let out_root = Path::new(&out_dir);
    let dir = out_root.join(alpha).join(hash);
    let data_root = data_dir.as_deref().map(Path::new).unwrap_or_else(|| {
        eprintln!("--data-dir <root> is required");
        std::process::exit(1);
    });

    // Either an explicit session, or the N worst — which is what you actually
    // want most of the time: you investigate the bad ones, not a random one.
    let sessions: Vec<String> = match (&session, worst) {
        (Some(s), _) => vec![s.clone()],
        (None, n) if n > 0 => worst_sessions(&dir, n),
        _ => {
            eprintln!("give --session <timeframe>/<market>/<id> or --worst N");
            std::process::exit(1);
        }
    };

    let mut failures = 0;
    for s in &sessions {
        let args = replay::Args {
            out_root,
            data_root,
            alpha,
            hash,
            session: s,
            code_hash: e.code_hash,
            no_cache,
        };
        match (e.replay)(args) {
            Ok(result) if result.cached => println!(
                "{s:<20} -- already complete; replay skipped -> {}",
                result.path.display()
            ),
            Ok(result) => println!("{s:<20} -- replay complete -> {}", result.path.display()),
            Err(msg) => {
                eprintln!("{s:<20} !! {msg}");
                failures += 1;
            }
        }
    }
    if failures > 0 {
        std::process::exit(1);
    }
}

/// The N worst sessions by pnl — read straight from tier A, no re-run.
fn worst_sessions(dir: &Path, n: usize) -> Vec<String> {
    let mut rows: Vec<(String, SessionRow)> = Vec::new();
    for tenor in fs::read_dir(dir).into_iter().flatten().flatten() {
        let t = tenor.path();
        if !t.is_dir() || t.file_name().is_some_and(|x| x == "replay") {
            continue;
        }
        for f in fs::read_dir(&t).into_iter().flatten().flatten() {
            let p = f.path();
            if p.extension().is_none_or(|x| x != "json") {
                continue;
            }
            if let Some(mut r) = fs::read_to_string(&p)
                .ok()
                .and_then(|s| serde_json::from_str::<Vec<SessionRow>>(&s).ok())
            {
                let market = format!(
                    "{}/{}",
                    t.file_name().unwrap().to_string_lossy(),
                    p.file_stem().unwrap().to_string_lossy()
                );
                rows.extend(r.drain(..).map(|row| (market.clone(), row)));
            }
        }
    }
    rows.retain(|(_, row)| row.error.is_none());
    rows.sort_by(|(_, a), (_, b)| a.pnl.total_cmp(&b.pnl));
    rows.into_iter()
        .take(n)
        .map(|(market, row)| format!("{market}/{}", row.session_id))
        .collect()
}
