use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    config::Config,
    model::{IntervalFile, RunRecord, RunStatus, ScenarioDraft},
    results,
    store::{now, Store},
};

struct App {
    config: Config,
    store: Store,
    active: Mutex<HashMap<String, u32>>,
}

type Response = (u16, &'static str, Vec<u8>);

fn json_ok<T: serde::Serialize>(value: T) -> Response {
    (
        200,
        "application/json; charset=utf-8",
        serde_json::to_vec(&value).unwrap(),
    )
}
fn error(status: u16, message: impl ToString) -> Response {
    (
        status,
        "application/json; charset=utf-8",
        json!({"error": message.to_string()})
            .to_string()
            .into_bytes(),
    )
}

fn interval_files(config: &Config, symbol: &str) -> Result<Vec<IntervalFile>, String> {
    if symbol.is_empty() || symbol.contains('/') || symbol.contains("..") {
        return Err("invalid symbol".into());
    }
    let dir = config.data_dir.join(&config.timeframe).join(symbol);
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|v| v == "npz")
                && !p.file_name().unwrap().to_string_lossy().starts_with('.')
        })
        .collect();
    files.sort();
    Ok(files
        .into_iter()
        .map(|path| IntervalFile {
            id: path.file_stem().unwrap().to_string_lossy().into_owned(),
            file: path.file_name().unwrap().to_string_lossy().into_owned(),
            path: path.to_string_lossy().into_owned(),
            status: "ready",
        })
        .collect())
}

fn symbols(config: &Config) -> Vec<String> {
    let mut result: Vec<_> = fs::read_dir(config.data_dir.join(&config.timeframe))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    result.sort();
    result
}

fn params(draft: &crate::model::Scenario) -> Value {
    let input = &draft.strategies[0].inputs;
    let mut values = Vec::new();
    for seconds in &input.elapse_seconds {
        for qty in &input.slice_quantity {
            values.push(
                json!({"elapse_ns": (seconds * 1_000_000_000.0).round() as i64, "slice_qty": qty}),
            );
        }
    }
    json!({"strategy": "twap_sell", "params": values})
}

fn start_run(app: &Arc<App>, scenario_id: &str) -> Result<RunRecord, String> {
    if app.active.lock().unwrap().contains_key(scenario_id) {
        return Err("scenario already has a running attempt".into());
    }
    let mut scenario = app.store.get(scenario_id)?;
    let intervals = interval_files(&app.config, &scenario.order.symbol)?;
    if intervals.is_empty() {
        return Err("no available intervals for selected symbol".into());
    }
    let run_id = Uuid::new_v4().to_string();
    let generated = params(&scenario);
    let run_dir = app.store.run_dir(scenario_id, &run_id);
    fs::create_dir_all(&run_dir).map_err(|e| e.to_string())?;
    fs::write(
        run_dir.join("params.json"),
        serde_json::to_vec_pretty(&generated).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    let mut run = RunRecord {
        schema_version: 1,
        id: run_id.clone(),
        scenario_id: scenario_id.into(),
        status: RunStatus::Pending,
        scenario_snapshot: scenario.clone(),
        resolved_intervals: intervals.iter().map(|i| i.path.clone()).collect(),
        params: generated.clone(),
        started_at: None,
        completed_at: None,
        pid: None,
        error: None,
    };
    app.store.save_run(&run)?;
    scenario.last_attempt_run_id = Some(run_id.clone());
    scenario.updated_at = now();
    app.store.save(&scenario)?;

    let log = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(run_dir.join("run.log"))
        .map_err(|e| e.to_string())?;
    let stderr = log.try_clone().map_err(|e| e.to_string())?;
    let market_dir = app
        .config
        .data_dir
        .join(&app.config.timeframe)
        .join(&scenario.order.symbol);
    let output_dir = run_dir.join("results");
    let mut command = Command::new("cargo");
    command
        .current_dir(&app.config.workspace)
        .args(["run", "--bin", "execlab", "--", "twap_sell", "--data-dir"])
        .arg(&market_dir)
        .arg("--out")
        .arg(&output_dir)
        .arg("--params-file")
        .arg(run_dir.join("params.json"))
        .arg("--progress-file")
        .arg(run_dir.join("progress.json"))
        .arg("--initial-position")
        .arg(
            scenario
                .order
                .side
                .initial_position(scenario.order.quantity)
                .to_string(),
        )
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr));
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start execlab: {e}"))?;
    run.status = RunStatus::Running;
    run.started_at = Some(now());
    run.pid = Some(child.id());
    app.store.save_run(&run)?;
    app.active
        .lock()
        .unwrap()
        .insert(scenario_id.into(), child.id());

    let app = Arc::clone(app);
    let sid = scenario_id.to_string();
    let rid = run_id.clone();
    let expected_configs = generated["params"].as_array().unwrap().len();
    let expected_intervals = intervals.len();
    thread::spawn(move || {
        let waited = child.wait();
        let mut record = match app.store.get_run(&sid, &rid) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("trader_viz: {e}");
                return;
            }
        };
        app.active.lock().unwrap().remove(&sid);
        if matches!(record.status, RunStatus::Cancelled) {
            record.completed_at = Some(now());
            let _ = app.store.save_run(&record);
            return;
        }
        let validation = match waited {
            Ok(status) if status.success() => results::fully_successful(
                &app.store.run_dir(&sid, &rid).join("results"),
                expected_configs,
                expected_intervals,
            ),
            Ok(status) => Err(format!("execlab exited with {status}; see run.log")),
            Err(e) => Err(format!("cannot wait for execlab: {e}")),
        };
        record.completed_at = Some(now());
        record.pid = None;
        match validation {
            Ok(()) => {
                record.status = RunStatus::Completed;
                if let Ok(mut scenario) = app.store.get(&sid) {
                    scenario.latest_successful_run_id = Some(rid.clone());
                    scenario.updated_at = now();
                    let _ = app.store.save(&scenario);
                }
            }
            Err(message) => {
                record.status = RunStatus::Failed;
                record.error = Some(message);
            }
        }
        let _ = app.store.save_run(&record);
    });
    Ok(run)
}

fn query(target: &str) -> HashMap<String, String> {
    target
        .split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.into(), v.replace("%2F", "/").replace("%20", " ")))
        .collect()
}

fn route(app: &Arc<App>, method: &str, target: &str, body: &[u8]) -> Response {
    let path = target.split('?').next().unwrap_or(target);
    if method == "GET" && path == "/" {
        return (
            200,
            "text/html; charset=utf-8",
            crate::ui::PAGE.as_bytes().to_vec(),
        );
    }
    if method == "GET" && path == "/api/meta" {
        return json_ok(
            json!({"timeframe": app.config.timeframe, "data_dir": app.config.data_dir, "symbols": symbols(&app.config)}),
        );
    }
    if method == "GET" && path == "/api/intervals" {
        return match query(target).get("symbol") {
            Some(symbol) => interval_files(&app.config, symbol)
                .map(json_ok)
                .unwrap_or_else(|e| error(400, e)),
            None => error(400, "missing symbol"),
        };
    }
    if method == "GET" && path == "/api/scenarios" {
        let scenarios: Vec<_> = app
            .store
            .list()
            .into_iter()
            .map(|scenario| {
                let best = scenario
                    .latest_successful_run_id
                    .as_ref()
                    .and_then(|run| {
                        results::load(&app.store.run_dir(&scenario.id, run).join("results")).ok()
                    })
                    .and_then(|report| {
                        results::best_strategy(&report).map(|strategy| {
                            json!({
                                "hash": strategy.hash,
                                "params": strategy.params,
                                "completion_pct": strategy.stats.completion_pct,
                                "mean_is_pct": strategy.stats.mean_is_pct,
                                "p15_is_pct": strategy.stats.p15_is_pct,
                                "p50_is_pct": strategy.stats.p50_is_pct,
                                "p90_is_pct": strategy.stats.p90_is_pct,
                                "fees": strategy.stats.fees,
                                "avg_trades": strategy.stats.avg_trades,
                                "avg_mean_divergence": strategy.stats.avg_mean_divergence,
                                "avg_max_divergence": strategy.stats.avg_max_divergence,
                                "status_pct": strategy.stats.status_pct,
                            })
                        })
                    });
                let mut value = serde_json::to_value(scenario).unwrap();
                value["best_strategy"] = best.unwrap_or(Value::Null);
                value
            })
            .collect();
        return json_ok(scenarios);
    }

    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    if method == "POST" && path == "/api/scenarios" {
        return serde_json::from_slice::<ScenarioDraft>(body)
            .map_err(|e| e.to_string())
            .and_then(|d| app.store.create(d))
            .map(json_ok)
            .unwrap_or_else(|e| error(400, e));
    }
    if parts.len() >= 3 && parts[0] == "api" && parts[1] == "scenarios" {
        let id = parts[2];
        if parts.len() == 3 && method == "GET" {
            return app
                .store
                .get(id)
                .map(json_ok)
                .unwrap_or_else(|e| error(404, e));
        }
        if parts.len() == 3 && method == "PUT" {
            return serde_json::from_slice::<ScenarioDraft>(body)
                .map_err(|e| e.to_string())
                .and_then(|d| app.store.update(id, d))
                .map(json_ok)
                .unwrap_or_else(|e| error(400, e));
        }
        if parts.len() == 3 && method == "DELETE" {
            return app
                .store
                .delete(id)
                .map(|_| json_ok(json!({"deleted":true})))
                .unwrap_or_else(|e| error(400, e));
        }
        if parts.get(3) == Some(&"duplicate") && method == "POST" {
            return app
                .store
                .duplicate(id)
                .map(json_ok)
                .unwrap_or_else(|e| error(400, e));
        }
        if parts.get(3) == Some(&"run") && method == "POST" {
            return start_run(app, id)
                .map(json_ok)
                .unwrap_or_else(|e| error(400, e));
        }
        if parts.get(3) == Some(&"latest-run") && method == "GET" {
            let result = app
                .store
                .get(id)
                .and_then(|s| s.last_attempt_run_id.ok_or("scenario has no runs".into()))
                .and_then(|run_id| {
                    let run = app.store.get_run(id, &run_id)?;
                    let expected_total = run
                        .params
                        .get("params")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len)
                        * run.resolved_intervals.len();
                    let progress_path = app.store.run_dir(id, &run_id).join("progress.json");
                    let progress = fs::read_to_string(progress_path)
                        .ok()
                        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                        .unwrap_or_else(|| json!({
                            "total": expected_total,
                            "completed": 0,
                            "failed": 0,
                            "running": 0,
                            "pending": expected_total,
                            "processed": 0,
                            "percent": 0.0,
                            "elapsed_sec": run.started_at.map_or(0, |started| now().saturating_sub(started)),
                            "eta_sec": 0.0,
                            "rate_per_sec": 0.0,
                            "finished": false,
                        }));
                    let mut value = serde_json::to_value(run).map_err(|e| e.to_string())?;
                    value["progress"] = progress;
                    Ok(value)
                });
            return result.map(json_ok).unwrap_or_else(|e| error(404, e));
        }
        if parts.get(3) == Some(&"performance") && method == "GET" {
            return app
                .store
                .get(id)
                .and_then(|s| {
                    s.latest_successful_run_id
                        .ok_or("scenario has no successful result".into())
                })
                .and_then(|r| results::load(&app.store.run_dir(id, &r).join("results")))
                .map(json_ok)
                .unwrap_or_else(|e| error(404, e));
        }
        if parts.get(3) == Some(&"cancel") && method == "POST" {
            let pid = app.active.lock().unwrap().remove(id);
            return match pid {
                Some(pid) => {
                    let _ = Command::new("kill")
                        .arg("-TERM")
                        .arg(pid.to_string())
                        .status();
                    let result = app
                        .store
                        .get(id)
                        .and_then(|s| s.last_attempt_run_id.ok_or("no active run".into()))
                        .and_then(|rid| {
                            let mut run = app.store.get_run(id, &rid)?;
                            run.status = RunStatus::Cancelled;
                            run.completed_at = Some(now());
                            run.pid = None;
                            app.store.save_run(&run)?;
                            Ok(run)
                        });
                    result.map(json_ok).unwrap_or_else(|e| error(400, e))
                }
                None => error(400, "scenario has no active run"),
            };
        }
    }
    if path == "/api/replay" {
        let q = query(target);
        let sid = q.get("scenario");
        let hash = q.get("hash");
        let session = q.get("session");
        let target = || -> Result<(PathBuf, String, String), String> {
            let sid = sid.ok_or("missing scenario")?;
            let scenario = app.store.get(sid)?;
            let run = scenario
                .latest_successful_run_id
                .ok_or("no successful run")?;
            let hash = hash.ok_or("missing hash")?.to_string();
            let session = session.ok_or("missing session")?.to_string();
            Ok((app.store.run_dir(sid, &run).join("results"), hash, session))
        }();
        let (out, hash, session) = match target {
            Ok(v) => v,
            Err(e) => return error(400, e),
        };
        let replay = out
            .join("twap_sell")
            .join(&hash)
            .join("replay")
            .join(&app.config.timeframe)
            .join(q.get("symbol").cloned().unwrap_or_default())
            .join(format!("{session}.json"));
        if method == "GET" {
            return fs::read(&replay)
                .map(|b| (200, "application/json; charset=utf-8", b))
                .unwrap_or_else(|_| error(404, "replay output does not exist"));
        }
        if method == "POST" {
            let selector = format!(
                "{}/{}/{}",
                app.config.timeframe,
                q.get("symbol").cloned().unwrap_or_default(),
                session
            );
            let output = Command::new("cargo")
                .current_dir(&app.config.workspace)
                .args(["run", "--bin", "exec_replay", "--"])
                .arg(format!("twap_sell/{hash}"))
                .arg("--data-dir")
                .arg(&app.config.data_dir)
                .arg("--out")
                .arg(&out)
                .arg("--session")
                .arg(selector)
                .arg("--no-cache")
                .output();
            return match output {
                Ok(v) if v.status.success() => json_ok(json!({"complete":true})),
                Ok(v) => error(500, String::from_utf8_lossy(&v.stderr)),
                Err(e) => error(500, e),
            };
        }
    }
    error(404, "not found")
}

fn read_request(stream: &mut TcpStream) -> Result<(String, String, Vec<u8>), String> {
    let mut data = Vec::new();
    let mut buf = [0u8; 8192];
    let header_end;
    loop {
        let n = stream.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("closed request".into());
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            header_end = pos + 4;
            break;
        }
        if data.len() > 1024 * 1024 {
            return Err("headers too large".into());
        }
    }
    let head = String::from_utf8_lossy(&data[..header_end]);
    let mut lines = head.lines();
    let first = lines.next().ok_or("empty request")?;
    let mut p = first.split_whitespace();
    let method = p.next().ok_or("missing method")?.to_string();
    let target = p.next().ok_or("missing target")?.to_string();
    let length = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0);
    while data.len() < header_end + length {
        let n = stream.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }
    Ok((
        method,
        target,
        data[header_end..data.len().min(header_end + length)].to_vec(),
    ))
}

fn respond(mut stream: TcpStream, app: Arc<App>) {
    let response = match read_request(&mut stream) {
        Ok((m, t, b)) => route(&app, &m, &t, &b),
        Err(e) => error(400, e),
    };
    let status = match response.0 {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    };
    let head=format!("HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",response.0,status,response.1,response.2.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&response.2);
}

pub fn serve(config: Config) -> Result<(), String> {
    let store = Store::new(config.state_dir.clone())?;
    let bind = config.bind.clone();
    let app = Arc::new(App {
        config,
        store,
        active: Mutex::new(HashMap::new()),
    });
    let listener = TcpListener::bind(&bind).map_err(|e| format!("cannot bind {bind}: {e}"))?;
    println!("trader_viz: http://{bind}");
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let app = Arc::clone(&app);
                thread::spawn(move || respond(s, app));
            }
            Err(e) => eprintln!("trader_viz: {e}"),
        }
    }
    Ok(())
}
