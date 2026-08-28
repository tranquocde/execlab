//! Minimal localhost HTTP server for paged view-model access.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, RwLock},
    thread,
};

use serde::Serialize;
use serde_json::json;

use crate::{
    process::{self, Report},
    render::asset_comparison,
    replay_view::{self, DisplayMode},
};

type ReplayJobs = Mutex<HashMap<String, String>>;

pub(crate) struct EmbeddedView {
    report: RwLock<Report>,
    html: String,
    data_root: RwLock<PathBuf>,
    outputs: RwLock<Vec<String>>,
    jobs: Arc<ReplayJobs>,
}

impl EmbeddedView {
    pub(crate) fn new(outputs: Vec<String>, data_root: PathBuf, html: String) -> Self {
        let report = process::output_process(&outputs).unwrap_or_else(|error| Report {
            strategies: Vec::new(),
            warnings: vec![format!("results not available yet: {error}")],
        });
        Self {
            report: RwLock::new(report),
            html,
            data_root: RwLock::new(data_root),
            outputs: RwLock::new(outputs),
            jobs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn reconfigure(&self, output: PathBuf, data_root: PathBuf) {
        let outputs = vec![output.to_string_lossy().into_owned()];
        *self.outputs.write().unwrap() = outputs.clone();
        *self.data_root.write().unwrap() = data_root;
        *self.report.write().unwrap() =
            process::output_process(&outputs).unwrap_or_else(|error| Report {
                strategies: Vec::new(),
                warnings: vec![format!("results not available yet: {error}")],
            });
    }

    pub(crate) fn respond(&self, method: &str, target: &str) -> (u16, &'static str, Vec<u8>) {
        let mapped = if target == "/results" { "/" } else { target };
        let page_path = mapped.split('?').next().unwrap_or(mapped);
        if method == "GET"
            && matches!(
                page_path,
                "/" | "/api/meta" | "/asset-comparison" | "/session-replay"
            )
        {
            let outputs = self.outputs.read().unwrap().clone();
            let fresh = process::output_process(&outputs).unwrap_or_else(|error| Report {
                strategies: Vec::new(),
                warnings: vec![format!("waiting for sweep output: {error}")],
            });
            *self.report.write().unwrap() = fresh;
        }
        let report = self.report.read().unwrap();
        let data_root = self.data_root.read().unwrap();
        route(method, mapped, &report, &self.html, &data_root, &self.jobs)
    }
}

struct ReplayTarget {
    key: String,
    identity: String,
    alpha_hash: String,
    selector: String,
    output_root: PathBuf,
    replay_file: PathBuf,
    interval: serde_json::Value,
}

fn replay_target(report: &Report, query: &HashMap<String, String>) -> Result<ReplayTarget, String> {
    let strategy_index = query.get("strategy").and_then(|value| value.parse().ok());
    let asset_index = query.get("asset").and_then(|value| value.parse().ok());
    let session_id = query.get("session").ok_or("missing session")?;
    let strategy = strategy_index
        .and_then(|index: usize| report.strategies.get(index))
        .ok_or("invalid strategy")?;
    let asset = asset_index
        .and_then(|index: usize| strategy.assets.get(index))
        .ok_or("invalid asset")?;
    let interval = asset
        .intervals
        .iter()
        .find(|interval| interval.session_id == *session_id)
        .ok_or("invalid session")?;
    let strategy_dir = PathBuf::from(&strategy.source_path);
    let output_root = strategy_dir
        .parent()
        .and_then(Path::parent)
        .ok_or("invalid strategy output path")?
        .to_path_buf();
    let selector = format!("{}/{}/{}", asset.timeframe, asset.asset, session_id);
    let replay_file = strategy_dir
        .join("replay")
        .join(&asset.timeframe)
        .join(&asset.asset)
        .join(format!("{session_id}.json"));
    let alpha_hash = format!("{}/{}", strategy.alpha, strategy.hash);
    Ok(ReplayTarget {
        key: format!("{alpha_hash}/{selector}"),
        identity: format!("{alpha_hash} — {selector}"),
        alpha_hash,
        selector,
        output_root,
        replay_file,
        interval: serde_json::to_value(interval)
            .map_err(|error| format!("cannot serialize interval: {error}"))?,
    })
}

#[derive(Serialize)]
struct Page<T> {
    items: Vec<T>,
    page: usize,
    page_size: usize,
    total: usize,
    pages: usize,
}

fn page_bounds(total: usize, query: &HashMap<String, String>) -> (usize, usize, usize, usize) {
    let size = query
        .get("size")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|size| matches!(*size, 5 | 10 | 20 | 50))
        .unwrap_or(5);
    let pages = total.div_ceil(size).max(1);
    let requested = query
        .get("page")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1);
    let page = requested.clamp(1, pages);
    let start = ((page - 1) * size).min(total);
    let end = (start + size).min(total);
    (page, size, start, end)
}

fn parse_target(target: &str) -> (&str, HashMap<String, String>) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    (path, query)
}

fn json_response(value: serde_json::Value) -> (u16, &'static str, Vec<u8>) {
    (
        200,
        "application/json; charset=utf-8",
        value.to_string().into_bytes(),
    )
}

fn json_error(status: u16, error: String) -> (u16, &'static str, Vec<u8>) {
    (
        status,
        "application/json; charset=utf-8",
        json!({"error": error}).to_string().into_bytes(),
    )
}

fn value_at<'a>(value: &'a serde_json::Value, path: &str) -> &'a serde_json::Value {
    path.split('.').fold(value, |current, key| &current[key])
}

fn compare_json(left: &serde_json::Value, right: &serde_json::Value) -> Ordering {
    match (left, right) {
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => a
            .as_f64()
            .unwrap_or(f64::NAN)
            .total_cmp(&b.as_f64().unwrap_or(f64::NAN)),
        (serde_json::Value::String(a), serde_json::Value::String(b)) => a.cmp(b),
        (serde_json::Value::Bool(a), serde_json::Value::Bool(b)) => a.cmp(b),
        (serde_json::Value::Null, serde_json::Value::Null) => Ordering::Equal,
        (serde_json::Value::Null, _) => Ordering::Greater,
        (_, serde_json::Value::Null) => Ordering::Less,
        (a, b) => a.to_string().cmp(&b.to_string()),
    }
}

fn sort_values(
    items: &mut [serde_json::Value],
    query: &HashMap<String, String>,
    columns: &[(&str, &str)],
) {
    let Some(path) = query
        .get("sort")
        .and_then(|requested| columns.iter().find(|(name, _)| name == requested))
        .map(|(_, path)| *path)
    else {
        return;
    };
    let descending = query.get("order").is_some_and(|order| order == "desc");
    items.sort_by(|left, right| {
        let order = compare_json(value_at(left, path), value_at(right, path));
        if descending {
            order.reverse()
        } else {
            order
        }
    });
}

fn route(
    method: &str,
    target: &str,
    report: &Report,
    html: &str,
    data_root: &Path,
    jobs: &Arc<ReplayJobs>,
) -> (u16, &'static str, Vec<u8>) {
    let (path, query) = parse_target(target);
    match path {
        "/" => (200, "text/html; charset=utf-8", html.as_bytes().to_vec()),
        "/asset-comparison" => (
            200,
            "text/html; charset=utf-8",
            asset_comparison::page().as_bytes().to_vec(),
        ),
        "/session-replay" => (
            200,
            "text/html; charset=utf-8",
            replay_view::replay(DisplayMode::Simple).as_bytes().to_vec(),
        ),
        "/api/replay-preview" => {
            let target = match replay_target(report, &query) {
                Ok(target) => target,
                Err(error) => return json_error(400, error),
            };
            let job = jobs.lock().unwrap().get(&target.key).cloned();
            let running = job.as_deref() == Some("running");
            let error = job.filter(|state| state != "running");
            if !target.replay_file.is_file() {
                return json_response(json!({
                    "identity": target.identity,
                    "interval": target.interval,
                    "exists": false,
                    "running": running,
                    "error": error,
                }));
            }
            match fs::read_to_string(&target.replay_file) {
                Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
                    Ok(replay) => json_response(json!({
                        "identity": target.identity,
                        "interval": target.interval,
                        "exists": true,
                        "running": running,
                        "error": error,
                        "basic": {
                            "alpha": replay["alpha"],
                            "hash": replay["hash"],
                            "code_hash": replay["code_hash"],
                            "market": replay["market"],
                            "session_id": replay["session_id"],
                            "source_file": replay["source_file"],
                            "max_inventory": replay["max_inventory"],
                            "n_maker": replay["n_maker"],
                            "n_taker": replay["n_taker"],
                            "samples_taken": replay["samples_taken"],
                        },
                        "verify": replay["verify"],
                    })),
                    Err(error) => json_error(500, format!("cannot parse replay: {error}")),
                },
                Err(error) => json_error(500, format!("cannot read replay: {error}")),
            }
        }
        "/api/replay-fills" => {
            let target = match replay_target(report, &query) {
                Ok(target) => target,
                Err(error) => return json_error(400, error),
            };
            let content = match fs::read_to_string(&target.replay_file) {
                Ok(content) => content,
                Err(error) => return json_error(404, format!("cannot read replay: {error}")),
            };
            let replay = match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(replay) => replay,
                Err(error) => return json_error(500, format!("cannot parse replay: {error}")),
            };
            let mut fills = match replay.get("fills").and_then(|fills| fills.as_array()) {
                Some(fills) => fills.clone(),
                None => return json_error(500, "replay has no fills array".into()),
            };
            sort_values(
                &mut fills,
                &query,
                &[
                    ("ts", "ts"),
                    ("ts_local", "ts_local"),
                    ("order_id", "order_id"),
                    ("side", "side"),
                    ("px", "px"),
                    ("qty", "qty"),
                    ("maker", "maker"),
                    ("position", "position"),
                    ("balance", "balance"),
                    ("fee", "fee"),
                ],
            );
            let (page, size, start, end) = page_bounds(fills.len(), &query);
            json_response(
                serde_json::to_value(Page {
                    items: fills[start..end].to_vec(),
                    page,
                    page_size: size,
                    total: fills.len(),
                    pages: fills.len().div_ceil(size).max(1),
                })
                .unwrap(),
            )
        }
        "/api/replay-fill-series" => {
            let target = match replay_target(report, &query) {
                Ok(target) => target,
                Err(error) => return json_error(400, error),
            };
            let content = match fs::read_to_string(&target.replay_file) {
                Ok(content) => content,
                Err(error) => return json_error(404, format!("cannot read replay: {error}")),
            };
            let replay = match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(replay) => replay,
                Err(error) => return json_error(500, format!("cannot parse replay: {error}")),
            };
            let mut points = replay
                .get("fills")
                .and_then(|fills| fills.as_array())
                .into_iter()
                .flatten()
                .filter_map(|fill| {
                    Some(json!({
                        "time_ms": fill.get("ts")?.as_i64()? / 1_000_000,
                        "price": fill.get("px")?.as_f64()?,
                        "side": fill.get("side"),
                        "maker": fill.get("maker"),
                        "qty": fill.get("qty"),
                    }))
                })
                .collect::<Vec<_>>();
            points.sort_by(|left, right| compare_json(&left["time_ms"], &right["time_ms"]));
            json_response(json!({"points": points}))
        }
        "/api/replay-curve-series" => {
            let target = match replay_target(report, &query) {
                Ok(target) => target,
                Err(error) => return json_error(400, error),
            };
            let content = match fs::read_to_string(&target.replay_file) {
                Ok(content) => content,
                Err(error) => return json_error(404, format!("cannot read replay: {error}")),
            };
            let replay = match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(replay) => replay,
                Err(error) => return json_error(500, format!("cannot parse replay: {error}")),
            };
            let mut points = replay
                .get("curve")
                .and_then(|curve| curve.as_array())
                .into_iter()
                .flatten()
                .filter_map(|sample| {
                    Some(json!({
                        "time_ms": sample.get("ts")?.as_i64()? / 1_000_000,
                        "bid": sample.get("bid"),
                        "ask": sample.get("ask"),
                        "effective_bid": sample.get("effective_bid"),
                        "effective_ask": sample.get("effective_ask"),
                        "bids": sample.get("bids"),
                        "asks": sample.get("asks"),
                        "effective_bids": sample.get("effective_bids"),
                        "effective_asks": sample.get("effective_asks"),
                        "position": sample.get("position"),
                        "balance": sample.get("balance"),
                        "fee": sample.get("fee"),
                        "equity": sample.get("equity"),
                    }))
                })
                .collect::<Vec<_>>();
            points.sort_by(|left, right| compare_json(&left["time_ms"], &right["time_ms"]));
            json_response(json!({"points": points}))
        }
        "/api/replay-curve" => {
            let target = match replay_target(report, &query) {
                Ok(target) => target,
                Err(error) => return json_error(400, error),
            };
            let content = match fs::read_to_string(&target.replay_file) {
                Ok(content) => content,
                Err(error) => return json_error(404, format!("cannot read replay: {error}")),
            };
            let replay = match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(replay) => replay,
                Err(error) => return json_error(500, format!("cannot parse replay: {error}")),
            };
            let mut curve = match replay.get("curve").and_then(|curve| curve.as_array()) {
                Some(curve) => curve.clone(),
                None => return json_error(500, "replay has no curve array".into()),
            };
            sort_values(
                &mut curve,
                &query,
                &[
                    ("ts", "ts"),
                    ("bid", "bid"),
                    ("ask", "ask"),
                    ("effective_bid", "effective_bid"),
                    ("effective_ask", "effective_ask"),
                    ("my_bid", "my_bid"),
                    ("my_ask", "my_ask"),
                    ("position", "position"),
                    ("balance", "balance"),
                    ("fee", "fee"),
                    ("equity", "equity"),
                ],
            );
            let (page, size, start, end) = page_bounds(curve.len(), &query);
            json_response(
                serde_json::to_value(Page {
                    items: curve[start..end].to_vec(),
                    page,
                    page_size: size,
                    total: curve.len(),
                    pages: curve.len().div_ceil(size).max(1),
                })
                .unwrap(),
            )
        }
        "/api/replay-run" => {
            if method != "POST" {
                return json_error(405, "POST required".into());
            }
            let target = match replay_target(report, &query) {
                Ok(target) => target,
                Err(error) => return json_error(400, error),
            };
            {
                let mut states = jobs.lock().unwrap();
                if states
                    .get(&target.key)
                    .is_some_and(|state| state == "running")
                {
                    return json_response(json!({"started": false, "running": true}));
                }
                states.insert(target.key.clone(), "running".into());
            }
            let jobs = Arc::clone(jobs);
            let data_root = data_root.to_path_buf();
            thread::spawn(move || {
                eprintln!("[execviz replay] starting {}", target.identity);
                let sibling = std::env::current_exe()
                    .ok()
                    .and_then(|path| path.parent().map(|parent| parent.join("exec_replay")));
                let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
                let output = if workspace.join("Cargo.toml").is_file() {
                    eprintln!("[execviz replay] using cargo-managed exec_replay");
                    Command::new("cargo")
                        .current_dir(workspace)
                        .args(["run", "--bin", "exec_replay", "--"])
                        .arg(&target.alpha_hash)
                        .arg("--data-dir")
                        .arg(&data_root)
                        .arg("--out")
                        .arg(&target.output_root)
                        .arg("--session")
                        .arg(&target.selector)
                        .arg("--no-cache")
                        .output()
                } else if sibling.as_ref().is_some_and(|path| path.is_file()) {
                    eprintln!(
                        "[execviz replay] using sibling {}",
                        sibling.as_ref().unwrap().display()
                    );
                    Command::new(sibling.unwrap())
                        .arg(&target.alpha_hash)
                        .arg("--data-dir")
                        .arg(&data_root)
                        .arg("--out")
                        .arg(&target.output_root)
                        .arg("--session")
                        .arg(&target.selector)
                        .arg("--no-cache")
                        .output()
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "neither Cargo workspace nor sibling exec_replay is available",
                    ))
                };
                let state = match output {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        if !stdout.trim().is_empty() {
                            eprintln!(
                                "[execviz replay] stdout for {}:\n{}",
                                target.identity,
                                stdout.trim()
                            );
                        }
                        if !stderr.trim().is_empty() {
                            eprintln!(
                                "[execviz replay] stderr for {}:\n{}",
                                target.identity,
                                stderr.trim()
                            );
                        }
                        if output.status.success() {
                            eprintln!("[execviz replay] complete: {}", target.identity);
                            "complete".into()
                        } else {
                            let state = format!("failed with {}: {}", output.status, stderr.trim());
                            eprintln!("[execviz replay] {state} ({})", target.identity);
                            state
                        }
                    }
                    Err(error) => {
                        let state = format!("failed to start exec_replay: {error}");
                        eprintln!("[execviz replay] {state} ({})", target.identity);
                        state
                    }
                };
                jobs.lock().unwrap().insert(target.key, state);
            });
            json_response(json!({"started": true, "running": true}))
        }
        "/api/meta" => {
            let assets = report
                .strategies
                .iter()
                .flat_map(|strategy| &strategy.assets)
                .map(|asset| (&asset.timeframe, &asset.asset))
                .collect::<HashSet<_>>();
            let intervals = report
                .strategies
                .iter()
                .flat_map(|strategy| &strategy.assets)
                .flat_map(|asset| {
                    asset
                        .intervals
                        .iter()
                        .map(|interval| (&asset.timeframe, &asset.asset, &interval.session_id))
                })
                .collect::<HashSet<_>>();
            json_response(json!({
                "strategies": report.strategies.len(),
                "assets": assets.len(),
                "intervals": intervals.len(),
                "warnings": report.warnings,
            }))
        }
        "/api/strategies" => {
            let (page, size, start, end) = page_bounds(report.strategies.len(), &query);
            let mut items: Vec<_> = report
                .strategies
                .iter()
                .enumerate()
                .map(|(offset, strategy)| {
                    json!({
                        "index": offset,
                        "name": format!("{} / {}", strategy.alpha, strategy.hash),
                        "alpha": strategy.alpha,
                        "hash": strategy.hash,
                        "source_path": strategy.source_path,
                        "params": strategy.params,
                        "stats": strategy.stats,
                    })
                })
                .collect();
            sort_values(
                &mut items,
                &query,
                &[
                    ("strategy", "name"),
                    ("mean_cost", "stats.mean_cost"),
                    ("p15_cost", "stats.p15_cost"),
                    ("p50_cost", "stats.p50_cost"),
                    ("p90_cost", "stats.p90_cost"),
                    ("completion", "stats.completion_pct"),
                    ("avg_mean_divergence", "stats.avg_mean_divergence"),
                    ("avg_max_divergence", "stats.avg_max_divergence"),
                    ("fees", "stats.fees"),
                    ("status", "stats.status_pct"),
                ],
            );
            json_response(
                serde_json::to_value(Page {
                    items: items[start..end].to_vec(),
                    page,
                    page_size: size,
                    total: report.strategies.len(),
                    pages: report.strategies.len().div_ceil(size).max(1),
                })
                .unwrap(),
            )
        }
        "/api/assets" => {
            let strategy_index = query.get("strategy").and_then(|v| v.parse().ok());
            let Some(strategy) = strategy_index.and_then(|i: usize| report.strategies.get(i))
            else {
                return (
                    400,
                    "text/plain; charset=utf-8",
                    b"invalid strategy".to_vec(),
                );
            };
            let (page, size, start, end) = page_bounds(strategy.assets.len(), &query);
            let mut items: Vec<_> = strategy
                .assets
                .iter()
                .enumerate()
                .map(|(offset, asset)| {
                    json!({
                        "index": offset,
                        "timeframe": asset.timeframe,
                        "asset": asset.asset,
                        "ok": asset.ok,
                        "stats": asset.stats,
                    })
                })
                .collect();
            sort_values(
                &mut items,
                &query,
                &[
                    ("asset", "asset"),
                    ("runs", "stats.runs"),
                    ("mean_cost", "stats.mean_cost"),
                    ("p15_cost", "stats.p15_cost"),
                    ("p50_cost", "stats.p50_cost"),
                    ("p90_cost", "stats.p90_cost"),
                    ("avg_mean_divergence", "stats.avg_mean_divergence"),
                    ("avg_max_divergence", "stats.avg_max_divergence"),
                    ("completion", "stats.completion_pct"),
                    ("fees", "stats.fees"),
                    ("status", "stats.status_pct"),
                ],
            );
            json_response(
                serde_json::to_value(Page {
                    items: items[start..end].to_vec(),
                    page,
                    page_size: size,
                    total: strategy.assets.len(),
                    pages: strategy.assets.len().div_ceil(size).max(1),
                })
                .unwrap(),
            )
        }
        "/api/intervals" => {
            let strategy_index = query.get("strategy").and_then(|v| v.parse().ok());
            let asset_index = query.get("asset").and_then(|v| v.parse().ok());
            let Some(asset) = strategy_index
                .and_then(|i: usize| report.strategies.get(i))
                .and_then(|strategy| asset_index.and_then(|i: usize| strategy.assets.get(i)))
            else {
                return (
                    400,
                    "text/plain; charset=utf-8",
                    b"invalid strategy or asset".to_vec(),
                );
            };
            let (page, size, start, end) = page_bounds(asset.intervals.len(), &query);
            let mut items = asset
                .intervals
                .iter()
                .map(|interval| serde_json::to_value(interval).unwrap())
                .collect::<Vec<_>>();
            sort_values(
                &mut items,
                &query,
                &[
                    ("interval", "session_id"),
                    ("start_position", "start_position"),
                    ("final_balance", "final_balance"),
                    ("num_orders", "num_orders"),
                    ("num_trades", "num_trades"),
                    ("n_maker", "n_maker"),
                    ("avg_filled_price", "avg_filled_price"),
                    ("percent_filled", "percent_filled"),
                    ("cost", "cost"),
                    ("mean_divergence", "mean_divergence_score"),
                    ("max_divergence", "max_divergence_score"),
                    ("completion", "completion_pct"),
                    ("fees", "fees"),
                    ("status", "status_ok"),
                ],
            );
            json_response(
                serde_json::to_value(Page {
                    items: items[start..end].to_vec(),
                    page,
                    page_size: size,
                    total: asset.intervals.len(),
                    pages: asset.intervals.len().div_ceil(size).max(1),
                })
                .unwrap(),
            )
        }
        "/api/asset-series" => {
            let strategy_index = query.get("strategy").and_then(|value| value.parse().ok());
            let asset_index = query.get("asset").and_then(|value| value.parse().ok());
            let Some(asset) = strategy_index
                .and_then(|index: usize| report.strategies.get(index))
                .and_then(|strategy| {
                    asset_index.and_then(|index: usize| strategy.assets.get(index))
                })
            else {
                return (
                    400,
                    "text/plain; charset=utf-8",
                    b"invalid strategy or asset".to_vec(),
                );
            };
            let points = asset
                .intervals
                .iter()
                .filter_map(|interval| {
                    interval.session_start_ts.map(|timestamp| {
                        json!({
                            "time_ms": timestamp / 1_000_000,
                            "is_pct": interval.cost,
                            "filled_cost_pct": interval.filled_cost_pct,
                            "residual_cost_pct": interval.residual_cost_pct,
                            "completion_pct": interval.completion_pct,
                            "fees": interval.fees,
                        })
                    })
                })
                .collect::<Vec<_>>();
            json_response(json!({
                "asset": format!("{}/{}", asset.timeframe, asset.asset.to_uppercase()),
                "points": points,
            }))
        }
        "/api/source" => {
            let strategy_index = query.get("strategy").and_then(|v| v.parse().ok());
            let Some(strategy) = strategy_index.and_then(|i: usize| report.strategies.get(i))
            else {
                return (
                    400,
                    "text/plain; charset=utf-8",
                    b"invalid strategy".to_vec(),
                );
            };
            json_response(json!({
                "name": format!("{} / {}", strategy.alpha, strategy.hash),
                "params": strategy.params,
                "backtest_config": strategy.backtest_config,
                "source": strategy.strategy_source,
            }))
        }
        _ => (404, "text/plain; charset=utf-8", b"not found".to_vec()),
    }
}

fn handle(
    mut stream: TcpStream,
    report: Arc<RwLock<Report>>,
    html: Arc<String>,
    data_root: Arc<PathBuf>,
    outputs: Arc<Vec<String>>,
    jobs: Arc<ReplayJobs>,
) {
    let mut buffer = [0u8; 8192];
    let Ok(read) = stream.read(&mut buffer) else {
        return;
    };
    let request = String::from_utf8_lossy(&buffer[..read]);
    let Some(request_line) = request.lines().next() else {
        return;
    };
    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return;
    };
    let page_path = target.split('?').next().unwrap_or(target);
    if method == "GET" && matches!(page_path, "/" | "/asset-comparison" | "/session-replay") {
        match process::output_process(&outputs) {
            Ok(fresh) => *report.write().unwrap() = fresh,
            Err(error) => eprintln!("execviz refresh failed; keeping previous data: {error}"),
        }
    }
    let report = report.read().unwrap();
    let (status, content_type, body) = route(method, target, &report, &html, &data_root, &jobs);
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "Bad Request",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&body);
}

pub fn serve(
    bind: &str,
    report: Report,
    html: String,
    data_root: PathBuf,
    outputs: Vec<String>,
) -> Result<(), String> {
    let listener =
        TcpListener::bind(bind).map_err(|error| format!("cannot bind {bind}: {error}"))?;
    println!("execviz serving at http://{bind}");
    println!("press Ctrl+C to stop");
    let report = Arc::new(RwLock::new(report));
    let html = Arc::new(html);
    let data_root = Arc::new(data_root);
    let outputs = Arc::new(outputs);
    let jobs: Arc<ReplayJobs> = Arc::new(Mutex::new(HashMap::new()));
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let report = Arc::clone(&report);
                let html = Arc::clone(&html);
                let data_root = Arc::clone(&data_root);
                let outputs = Arc::clone(&outputs);
                let jobs = Arc::clone(&jobs);
                thread::spawn(move || handle(stream, report, html, data_root, outputs, jobs));
            }
            Err(error) => eprintln!("execviz connection error: {error}"),
        }
    }
    Ok(())
}
