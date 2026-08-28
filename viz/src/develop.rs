use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use serde_json::json;

use crate::server::EmbeddedView;

const ALPHA_TEMPLATE: &str = r#"use hftbacktest::prelude::*;
use serde::{Deserialize, Serialize};

use crate::alpha::Alpha;

pub struct A;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Params {
    pub elapse_ns: i64,
}

impl Alpha for A {
    type Params = Params;

    fn search_space() -> Vec<Params> {
        vec![Params { elapse_ns: 10_000_000 }]
    }

    fn run<MD, B>(hbt: &mut B, p: &Params)
    where
        MD: MarketDepth,
        B: Bot<MD>,
        B::Error: std::fmt::Debug,
    {
        while hbt.elapse(p.elapse_ns).unwrap() == ElapseResult::Ok {}
        hbt.close().unwrap();
    }
}
"#;

const CONFIG_TEMPLATE: &str = r#"{
  "data_dir": "data",
  "output_dir": "output",
  "backtest_config": {
    "exchange": "EffectivePartialFillExchange",
    "queue_model": "RiskAdverseQueueModel",
    "asset_type": "LinearAsset",
    "contract_size": 1.0,
    "tick_size": 0.01,
    "lot_size": 0.001,
    "latency_model": "ConstantLatency",
    "entry_latency_ns": 10000000,
    "response_latency_ns": 10000000,
    "fee_model": "TradingValueFeeModel<CommonFees>",
    "maker_fee": 0.005,
    "taker_fee": 0.005,
    "initial_balance": 0.0,
    "initial_position": 100000.0,
    "last_trades_capacity": 1000,
    "terminal_valuation": "BinarySettlement"
  }
}
"#;

#[derive(Clone)]
pub struct DevelopConfig {
    pub alpha: String,
    pub alpha_dir: PathBuf,
    pub workspace: PathBuf,
}

struct RunState {
    status: String,
    run_id: Option<String>,
    log_path: Option<PathBuf>,
    child: Option<Arc<Mutex<Child>>>,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            status: "idle".into(),
            run_id: None,
            log_path: None,
            child: None,
        }
    }
}

#[derive(Deserialize)]
struct SaveRequest {
    source: String,
    config: String,
}

#[derive(Deserialize)]
struct RunRequest {
    #[serde(default)]
    release: bool,
}

pub fn prepare_alpha(execs_dir: &Path, alpha: &str) -> Result<(PathBuf, bool), String> {
    if !valid_alpha_name(alpha) {
        return Err("alpha name must match [a-z][a-z0-9_]*".into());
    }
    let dir = execs_dir.join(alpha);
    let source = dir.join(format!("{alpha}.rs"));
    let config = dir.join(format!("{alpha}.json"));
    if dir.exists() {
        if !source.is_file() || !config.is_file() {
            return Err(format!(
                "incomplete alpha folder {}; expected {} and {}",
                dir.display(),
                source.display(),
                config.display()
            ));
        }
        return Ok((dir, false));
    }
    fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    fs::write(&source, ALPHA_TEMPLATE)
        .map_err(|error| format!("cannot write {}: {error}", source.display()))?;
    fs::write(&config, CONFIG_TEMPLATE)
        .map_err(|error| format!("cannot write {}: {error}", config.display()))?;
    Ok((dir, true))
}

fn valid_alpha_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn paths(config: &DevelopConfig) -> (PathBuf, PathBuf) {
    (
        config.alpha_dir.join(format!("{}.rs", config.alpha)),
        config.alpha_dir.join(format!("{}.json", config.alpha)),
    )
}

fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, content)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("cannot replace {}: {error}", path.display()))
}

fn save(config: &DevelopConfig, request: SaveRequest) -> Result<(), String> {
    let parsed: serde_json::Value = serde_json::from_str(&request.config)
        .map_err(|error| format!("invalid JSON config: {error}"))?;
    for field in ["data_dir", "output_dir", "backtest_config"] {
        if parsed.get(field).is_none() {
            return Err(format!("config is missing `{field}`"));
        }
    }
    let (source, config_path) = paths(config);
    atomic_write(&source, &request.source)?;
    atomic_write(
        &config_path,
        &serde_json::to_string_pretty(&parsed).unwrap(),
    )
}

fn run_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("{millis}-{:x}", std::process::id())
}

fn append_stream<R: Read + Send + 'static>(
    reader: R,
    log: Arc<Mutex<fs::File>>,
    label: &'static str,
) {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let mut file = log.lock().unwrap();
            let _ = writeln!(file, "[{label}] {line}");
            let _ = file.flush();
        }
    });
}

fn configured_roots(config: &DevelopConfig) -> Result<(PathBuf, PathBuf), String> {
    let (_, config_path) = paths(config);
    let value: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&config_path)
            .map_err(|error| format!("cannot read {}: {error}", config_path.display()))?,
    )
    .map_err(|error| format!("cannot parse {}: {error}", config_path.display()))?;
    let resolve = |field: &str| -> Result<PathBuf, String> {
        let path = value
            .get(field)
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("config `{field}` must be a non-empty string"))?;
        let path = PathBuf::from(path);
        Ok(if path.is_absolute() {
            path
        } else {
            config.workspace.join(path)
        })
    };
    Ok((resolve("data_dir")?, resolve("output_dir")?))
}

fn alpha_output_dir(config: &DevelopConfig) -> Result<PathBuf, String> {
    let (_, output_root) = configured_roots(config)?;
    Ok(output_root.join(&config.alpha))
}

fn output_differs_from_current(config: &DevelopConfig, output: &Path) -> Result<bool, String> {
    if !output.exists() {
        return Ok(false);
    }
    let (source_path, config_path) = paths(config);
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("cannot read {}: {error}", source_path.display()))?;
    let alpha_config: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&config_path)
            .map_err(|error| format!("cannot read {}: {error}", config_path.display()))?,
    )
    .map_err(|error| format!("cannot parse {}: {error}", config_path.display()))?;
    let expected = alpha_config
        .get("backtest_config")
        .ok_or("config is missing `backtest_config`")?;
    let entries = fs::read_dir(output)
        .map_err(|error| format!("cannot inspect {}: {error}", output.display()))?;
    for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
        let dir = entry.path();
        let manifest: serde_json::Value = match fs::read_to_string(dir.join("manifest.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
        {
            Some(manifest) => manifest,
            None => return Ok(true),
        };
        if manifest.get("backtest_config") != Some(expected) {
            return Ok(true);
        }
        let recorded_source = fs::read_to_string(dir.join("strategy.rs")).unwrap_or_default();
        if !recorded_source.contains(&source) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn start_run(
    config: &DevelopConfig,
    state: Arc<Mutex<RunState>>,
    force: bool,
    release: bool,
) -> Result<String, String> {
    if state.lock().unwrap().status == "running" {
        return Err("a sweep is already running".into());
    }
    let id = run_id();
    let run_dir = config.workspace.join(".execlab/runs").join(&id);
    fs::create_dir_all(&run_dir)
        .map_err(|error| format!("cannot create {}: {error}", run_dir.display()))?;
    let (_, alpha_config_path) = paths(config);
    fs::copy(&alpha_config_path, run_dir.join("spec.json"))
        .map_err(|error| format!("cannot snapshot config: {error}"))?;
    let log_path = run_dir.join("run.log");
    let log_file = fs::File::create(&log_path)
        .map_err(|error| format!("cannot create {}: {error}", log_path.display()))?;
    let log = Arc::new(Mutex::new(log_file));

    let alpha_output = alpha_output_dir(config)?;
    let config_changed = output_differs_from_current(config, &alpha_output)?;
    if alpha_output.exists() && (force || config_changed) {
        {
            let mut file = log.lock().unwrap();
            let reason = if force {
                "force run"
            } else {
                "strategy or backtest config changed"
            };
            let _ = writeln!(
                file,
                "[execviz] removing {} ({reason})",
                alpha_output.display()
            );
            let _ = file.flush();
        }
        fs::remove_dir_all(&alpha_output)
            .map_err(|error| format!("cannot remove {}: {error}", alpha_output.display()))?;
    }

    let mut command = Command::new("cargo");
    command.current_dir(&config.workspace).arg("run");
    if release {
        command.arg("--release");
    }
    command.args(["--bin", "execlab", "--", &config.alpha]);
    if force {
        command.arg("--force");
    }
    {
        let mut file = log.lock().unwrap();
        let profile = if release { "release" } else { "debug" };
        let force_arg = if force { " --force" } else { "" };
        let _ = writeln!(
            file,
            "[execviz] compile mode: {profile}\n[execviz] command: cargo run{} --bin execlab -- {}{}",
            if release { " --release" } else { "" },
            config.alpha,
            force_arg,
        );
        let _ = file.flush();
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start sweep: {error}"))?;
    append_stream(child.stdout.take().unwrap(), Arc::clone(&log), "stdout");
    append_stream(child.stderr.take().unwrap(), Arc::clone(&log), "stderr");
    let child = Arc::new(Mutex::new(child));
    {
        let mut current = state.lock().unwrap();
        current.status = "running".into();
        current.run_id = Some(id.clone());
        current.log_path = Some(log_path.clone());
        current.child = Some(Arc::clone(&child));
    }
    let monitor_state = Arc::clone(&state);
    let status_path = run_dir.join("status.json");
    let alpha = config.alpha.clone();
    let monitor_id = id.clone();
    thread::spawn(move || loop {
        let result = child.lock().unwrap().try_wait();
        match result {
            Ok(Some(exit)) => {
                let final_status = if exit.success() {
                    "completed"
                } else {
                    "failed"
                };
                let _ = fs::write(
                    &status_path,
                    json!({
                        "run_id": monitor_id, "alpha": alpha, "status": final_status,
                        "exit_code": exit.code(),
                        "compile_mode": if release { "release" } else { "debug" },
                    })
                    .to_string(),
                );
                let mut current = monitor_state.lock().unwrap();
                current.status = final_status.into();
                current.child = None;
                break;
            }
            Ok(None) => thread::sleep(Duration::from_millis(250)),
            Err(error) => {
                let mut file = log.lock().unwrap();
                let _ = writeln!(file, "[execviz] cannot monitor sweep: {error}");
                monitor_state.lock().unwrap().status = "failed".into();
                break;
            }
        }
    });
    Ok(id)
}

fn json_response(value: serde_json::Value) -> (u16, &'static str, Vec<u8>) {
    (
        200,
        "application/json; charset=utf-8",
        value.to_string().into_bytes(),
    )
}

fn error_response(status: u16, error: impl ToString) -> (u16, &'static str, Vec<u8>) {
    (
        status,
        "application/json; charset=utf-8",
        json!({"error": error.to_string()}).to_string().into_bytes(),
    )
}

fn route(
    method: &str,
    path: &str,
    body: &[u8],
    config: &DevelopConfig,
    state: Arc<Mutex<RunState>>,
    view: &EmbeddedView,
) -> (u16, &'static str, Vec<u8>) {
    let route_path = path.split('?').next().unwrap_or(path);
    match (method, route_path) {
        ("GET", "/") => (
            200,
            "text/html; charset=utf-8",
            PAGE.replace("__ALPHA__", &config.alpha).into_bytes(),
        ),
        ("GET", "/api/develop") => {
            let (source, config_path) = paths(config);
            let current = state.lock().unwrap();
            let log = current
                .log_path
                .as_ref()
                .and_then(|path| fs::read_to_string(path).ok())
                .unwrap_or_default();
            json_response(json!({
                "alpha": config.alpha,
                "source": fs::read_to_string(source).unwrap_or_default(),
                "config": fs::read_to_string(config_path).unwrap_or_default(),
                "status": current.status,
                "run_id": current.run_id,
                "log": log,
            }))
        }
        ("POST", "/api/save") => match serde_json::from_slice::<SaveRequest>(body) {
            Ok(request) => match save(config, request) {
                Ok(()) => match configured_roots(config) {
                    Ok((data_root, output_root)) => {
                        view.reconfigure(output_root, data_root);
                        json_response(json!({"ok": true}))
                    }
                    Err(error) => error_response(400, error),
                },
                Err(error) => error_response(400, error),
            },
            Err(error) => error_response(400, format!("invalid request: {error}")),
        },
        ("POST", "/api/run") | ("POST", "/api/run-force") => {
            // Empty body preserves the original release-mode API behavior.
            let release = if body.is_empty() {
                true
            } else {
                match serde_json::from_slice::<RunRequest>(body) {
                    Ok(request) => request.release,
                    Err(error) => {
                        return error_response(400, format!("invalid run request: {error}"))
                    }
                }
            };
            match start_run(config, state, route_path.ends_with("force"), release) {
                Ok(id) => json_response(json!({"ok": true, "run_id": id})),
                Err(error) => error_response(400, error),
            }
        }
        ("POST", "/api/stop") => {
            let child = state.lock().unwrap().child.clone();
            match child {
                Some(child) => match child.lock().unwrap().kill() {
                    Ok(()) => json_response(json!({"ok": true})),
                    Err(error) => error_response(500, error),
                },
                None => error_response(400, "no sweep is running"),
            }
        }
        _ => view.respond(method, path),
    }
}

fn read_request(stream: &mut TcpStream) -> Result<(String, String, Vec<u8>), String> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    let header_end;
    loop {
        let read = stream
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("incomplete request".into());
        }
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            header_end = index + 4;
            break;
        }
    }
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let mut parts = headers
        .lines()
        .next()
        .ok_or("missing request line")?
        .split_whitespace();
    let method = parts.next().ok_or("missing method")?.to_string();
    let path = parts.next().ok_or("missing path")?.to_string();
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    while bytes.len() < header_end + length {
        let read = stream
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    Ok((
        method,
        path,
        bytes[header_end..bytes.len().min(header_end + length)].to_vec(),
    ))
}

pub fn serve(bind: &str, config: DevelopConfig, view: EmbeddedView) -> Result<(), String> {
    let listener =
        TcpListener::bind(bind).map_err(|error| format!("cannot bind {bind}: {error}"))?;
    println!("execviz develop {} at http://{bind}", config.alpha);
    let config = Arc::new(config);
    let view = Arc::new(view);
    let state = Arc::new(Mutex::new(RunState::default()));
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let config = Arc::clone(&config);
        let state = Arc::clone(&state);
        let view = Arc::clone(&view);
        thread::spawn(move || {
            let Ok((method, path, body)) = read_request(&mut stream) else {
                return;
            };
            let (status, content_type, response) =
                route(&method, &path, &body, &config, state, &view);
            let reason = if status == 200 { "OK" } else { "Error" };
            let header = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", response.len());
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&response);
        });
    }
    Ok(())
}

const PAGE: &str = r##"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Develop __ALPHA__</title><style>
:root{--editor-font-size:13px;--editor-line-height:20px}*{box-sizing:border-box}body{margin:0;padding:18px;background:#f3f3f3;color:#111;font:15px/1.4 "Courier New",monospace}main{max-width:1800px;margin:auto;border:2px solid #222;background:#f7f7f7}header,.section{padding:16px 20px;border-bottom:2px solid #222}header{display:flex;justify-content:space-between;align-items:center}.brand{color:#6f2da8;font-size:28px;font-weight:700}.grid{display:grid;grid-template-columns:1fr 1fr}.panel{padding:16px 20px;min-width:0}.panel+.panel{border-left:2px solid #222}.panel-title{display:flex;justify-content:space-between;align-items:center;gap:12px}.font-controls{display:flex;align-items:center;gap:5px;font-size:12px}.font-controls button{min-width:34px;padding:3px 8px}.font-size{min-width:38px;text-align:center;color:#555}.code-editor{display:grid;grid-template-columns:52px minmax(0,1fr);height:520px;border:1px solid #222;background:#fbfbfb;overflow:hidden}.line-numbers{height:100%;margin:0;padding:12px 10px;background:#e8e8e8;color:#777;border-right:1px solid #bbb;text-align:right;font:var(--editor-font-size)/var(--editor-line-height) "Courier New",monospace;white-space:pre;overflow:hidden;user-select:none}.editor-body{position:relative;min-width:0;height:100%;overflow:hidden}.highlight,.code-input{position:absolute;inset:0;width:100%;height:100%;margin:0;padding:12px;border:0;font:var(--editor-font-size)/var(--editor-line-height) "Courier New",monospace;tab-size:4;white-space:pre;overflow:auto}.highlight{z-index:1;background:transparent;color:#222;pointer-events:none}.code-input{z-index:2;resize:none;outline:0;background:transparent;color:transparent;caret-color:#111;-webkit-text-fill-color:transparent}.code-input::selection{background:#b9d8ff}.tok-comment{color:#72806a;font-style:italic}.tok-string{color:#a31515}.tok-keyword{color:#7030a0;font-weight:700}.tok-type{color:#176b72}.tok-number{color:#1558a5}.tok-macro{color:#9b4d00}.tok-key{color:#7b2d8e;font-weight:700}.tok-literal{color:#1558a5;font-weight:700}.editor-body:focus-within{background:#fff;box-shadow:inset 0 0 0 1px #6f2da8}button{border:2px solid #222;background:#fff;font:inherit;font-weight:700;padding:6px 12px;cursor:pointer}.primary{background:#6f2da8;color:#fff;border-color:#6f2da8}.danger{color:#b00020}.actions{display:flex;gap:9px;align-items:center;flex-wrap:wrap}#log{height:260px;overflow:auto;background:#111;color:#eee;padding:12px;white-space:pre-wrap}.ok{color:#14833b}.bad{color:#b00020}@media(max-width:900px){.grid{grid-template-columns:1fr}.panel+.panel{border-left:0;border-top:2px solid #222}}
#log{width:100%;min-height:140px;resize:vertical}.log-title{display:flex;align-items:center;justify-content:space-between;gap:12px}.log-controls{display:flex;align-items:center;gap:6px}.log-controls button{padding:3px 10px;min-width:36px}.log-height{min-width:52px;text-align:center;color:#555;font-size:12px}
</style></head><body><main><header><div><h1>DEVELOP: __ALPHA__</h1><div>Edit, validate, run, inspect logs.</div></div><div class="brand">VPS Securities JSC</div></header><div class="grid"><section class="panel"><div class="panel-title"><h2>STRATEGY / SEARCH SPACE</h2><div class="font-controls"><button class="font-down" title="Decrease editor font">A−</button><span class="font-size">13px</span><button class="font-up" title="Increase editor font">A+</button></div></div><div class="code-editor"><pre id="source-lines" class="line-numbers" aria-hidden="true">1</pre><div class="editor-body"><pre id="source-highlight" class="highlight" aria-hidden="true"></pre><textarea id="source" class="code-input" spellcheck="false" autocomplete="off"></textarea></div></div></section><section class="panel"><div class="panel-title"><h2>BACKTEST CONFIG</h2><div class="font-controls"><button class="font-down" title="Decrease editor font">A−</button><span class="font-size">13px</span><button class="font-up" title="Increase editor font">A+</button></div></div><div class="code-editor"><pre id="config-lines" class="line-numbers" aria-hidden="true">1</pre><div class="editor-body"><pre id="config-highlight" class="highlight" aria-hidden="true"></pre><textarea id="config" class="code-input" spellcheck="false" autocomplete="off"></textarea></div></div></section></div><section class="section"><div class="actions"><button id="save" class="primary">Save</button><button id="run">Run</button><button id="force" class="danger">Force Run</button><button id="stop">Stop</button><button id="results">View Results</button><span id="status">Loading…</span></div></section><section class="section"><div class="log-title"><h2>RUN LOG</h2><div class="log-controls"><button id="log-down" title="Decrease log height">−</button><span id="log-height" class="log-height">260px</span><button id="log-up" title="Increase log height">+</button><button id="log-reset" title="Reset log height">Reset</button></div></div><pre id="log"></pre></section></main><script>
const source=document.getElementById('source'),config=document.getElementById('config'),statusEl=document.getElementById('status'),log=document.getElementById('log');let loaded=false;
document.getElementById('save').insertAdjacentHTML('beforebegin','<label style="display:flex;align-items:center;gap:6px;font-weight:700">Compile:<select id="compile-mode" style="border:2px solid #222;background:#fff;font:inherit;padding:5px 8px"><option value="debug">Debug (faster compile)</option><option value="release">Release (faster sweep)</option></select></label>');
const compileMode=document.getElementById('compile-mode');try{compileMode.value=localStorage.getItem('execviz-compile-mode')||'debug'}catch(_){}compileMode.onchange=()=>{try{localStorage.setItem('execviz-compile-mode',compileMode.value)}catch(_){}};
const runOptions=()=>({method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({release:compileMode.value==='release'})});
const defaultLogHeight=260;let logHeight=defaultLogHeight;try{logHeight=Number(localStorage.getItem('execviz-log-height'))||defaultLogHeight}catch(_){}
function setLogHeight(height,remember=true){logHeight=Math.max(140,Math.min(1400,height));log.style.height=logHeight+'px';document.getElementById('log-height').textContent=logHeight+'px';if(remember)try{localStorage.setItem('execviz-log-height',logHeight)}catch(_){}}
document.getElementById('log-down').onclick=()=>setLogHeight(logHeight-100);document.getElementById('log-up').onclick=()=>setLogHeight(logHeight+100);document.getElementById('log-reset').onclick=()=>setLogHeight(defaultLogHeight);setLogHeight(logHeight,false);
new ResizeObserver(()=>{const height=Math.round(log.getBoundingClientRect().height);if(Math.abs(height-logHeight)>1)setLogHeight(height)}).observe(log);
let editorFontSize=13;try{editorFontSize=Number(localStorage.getItem('execviz-editor-font-size'))||13}catch(_){}
function setEditorFont(size){editorFontSize=Math.max(10,Math.min(24,size));document.documentElement.style.setProperty('--editor-font-size',editorFontSize+'px');document.documentElement.style.setProperty('--editor-line-height',(editorFontSize+7)+'px');document.querySelectorAll('.font-size').forEach(label=>label.textContent=editorFontSize+'px');try{localStorage.setItem('execviz-editor-font-size',editorFontSize)}catch(_){}}
document.querySelectorAll('.font-down').forEach(button=>button.onclick=()=>setEditorFont(editorFontSize-1));document.querySelectorAll('.font-up').forEach(button=>button.onclick=()=>setEditorFont(editorFontSize+1));setEditorFont(editorFontSize);
const esc=s=>s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
function paint(text,mode){
 const rust=/\/\*[\s\S]*?\*\/|\/\/[^\n]*|r#*"[\s\S]*?"#*|b?"(?:\\.|[^"\\])*"|b?'(?:\\.|[^'\\])+'|\b(?:fn|let|mut|pub|struct|enum|impl|trait|type|where|use|mod|crate|self|Self|super|const|static|async|await|move|dyn|ref|match|if|else|while|loop|for|in|return|break|continue|as|unsafe|extern)\b|\b(?:String|Option|Result|Vec|Box|Path|PathBuf|f32|f64|i8|i16|i32|i64|i128|isize|u8|u16|u32|u64|u128|usize|bool|str)\b|\b(?:true|false|None|Some|Ok|Err)\b|\b(?:0x[\da-fA-F_]+|\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?)\b|\b[a-zA-Z_]\w*!/g;
 const json=/"(?:\\.|[^"\\])*"(?=\s*:)|"(?:\\.|[^"\\])*"|\b(?:true|false|null)\b|-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b/g;
 const regex=mode==='rust'?rust:json;let out='',last=0,match;
 while((match=regex.exec(text))){out+=esc(text.slice(last,match.index));const token=match[0];let cls;
  if(mode==='json')cls=/^"/.test(token)?(/"\s*$/.test(token)&&/^"(?:\\.|[^"\\])*"$/.test(token)&&/^\s*:/.test(text.slice(regex.lastIndex))?'tok-key':'tok-string'):/true|false|null/.test(token)?'tok-literal':'tok-number';
  else cls=/^\/\//.test(token)||/^\/\*/.test(token)?'tok-comment':/^(?:r#*"|b?"|b?')/.test(token)?'tok-string':/!$/.test(token)?'tok-macro':/^(?:String|Option|Result|Vec|Box|Path|PathBuf|f|i|u|bool|str)/.test(token)?'tok-type':/^(?:true|false|None|Some|Ok|Err)$/.test(token)?'tok-literal':/^\d|^0x/.test(token)?'tok-number':'tok-keyword';
  out+='<span class="'+cls+'">'+esc(token)+'</span>';last=regex.lastIndex;
 }
 return out+esc(text.slice(last))+(text.endsWith('\n')?' ':'');
}
function bindEditor(editor,gutter,highlight,mode){
 const update=()=>{const count=editor.value.split('\n').length;gutter.textContent=Array.from({length:count},(_,i)=>i+1).join('\n');highlight.innerHTML=paint(editor.value,mode)};
 const sync=()=>{gutter.scrollTop=editor.scrollTop;highlight.scrollTop=editor.scrollTop;highlight.scrollLeft=editor.scrollLeft};
 editor.addEventListener('input',update);editor.addEventListener('scroll',sync);editor.addEventListener('keydown',event=>{
  if(event.key==='Tab'){event.preventDefault();const a=editor.selectionStart,b=editor.selectionEnd;editor.setRangeText('    ',a,b,'end');editor.dispatchEvent(new Event('input'))}
  const pairs={'(' : ')','[':']','{':'}','"':'"',"'":"'"};if(pairs[event.key]&&editor.selectionStart===editor.selectionEnd){event.preventDefault();const p=editor.selectionStart;editor.setRangeText(event.key+pairs[event.key],p,p,'end');editor.selectionStart=editor.selectionEnd=p+1;editor.dispatchEvent(new Event('input'))}
 });return update;
}
const updateSource=bindEditor(source,document.getElementById('source-lines'),document.getElementById('source-highlight'),'rust');
const updateConfig=bindEditor(config,document.getElementById('config-lines'),document.getElementById('config-highlight'),'json');
async function request(path,options){const response=await fetch(path,options),data=await response.json();if(!response.ok)throw new Error(data.error||'request failed');return data}
async function refresh(){try{const data=await request('/api/develop');if(!loaded){source.value=data.source;config.value=data.config;updateSource();updateConfig();loaded=true}statusEl.textContent=data.status+(data.run_id?' — '+data.run_id:'');statusEl.className=data.status==='completed'?'ok':data.status==='failed'?'bad':'';log.textContent=data.log;log.scrollTop=log.scrollHeight;if(data.status==='running')setTimeout(refresh,750)}catch(error){statusEl.textContent=error.message;statusEl.className='bad'}}
async function save(){const data=JSON.stringify({source:source.value,config:config.value});await request('/api/save',{method:'POST',headers:{'Content-Type':'application/json'},body:data});statusEl.textContent='saved';statusEl.className='ok'}
document.getElementById('save').onclick=()=>save().catch(error=>{statusEl.textContent=error.message;statusEl.className='bad'});
document.getElementById('run').onclick=async()=>{try{await save();await request('/api/run',runOptions());refresh()}catch(error){statusEl.textContent=error.message;statusEl.className='bad'}};
document.getElementById('force').onclick=async()=>{if(!confirm('Force rerun the configured data scope?'))return;try{await save();await request('/api/run-force',runOptions());refresh()}catch(error){statusEl.textContent=error.message;statusEl.className='bad'}};
document.getElementById('stop').onclick=()=>request('/api/stop',{method:'POST'}).then(refresh).catch(error=>{statusEl.textContent=error.message;statusEl.className='bad'});
document.getElementById('results').onclick=()=>window.open('/results','_blank');refresh();
</script></body></html>"##;
