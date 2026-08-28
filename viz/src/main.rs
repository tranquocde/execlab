mod develop;
mod process;
mod render;
mod replay_view;
mod server;

use std::{env, fs, path::PathBuf};

use render::{html::HtmlRenderer, Renderer};

enum Mode {
    View,
    Develop { alpha: String, execs_dir: PathBuf },
}

struct Args {
    mode: Mode,
    outputs: Vec<String>,
    bind: String,
    data_dir: PathBuf,
}

fn parse_output_spec(spec: &str) -> Vec<String> {
    spec.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(str::trim)
        .map(|path| path.trim_matches(['\'', '"']))
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_args() -> Result<Args, String> {
    let mut raw = env::args().skip(1).collect::<Vec<_>>();
    let mut mode = Mode::View;
    if raw.first().is_some_and(|arg| arg == "view") {
        raw.remove(0);
    } else if raw.first().is_some_and(|arg| arg == "develop") {
        raw.remove(0);
        let alpha = raw
            .first()
            .cloned()
            .ok_or("develop requires <alpha_name>")?;
        raw.remove(0);
        mode = Mode::Develop {
            alpha,
            execs_dir: PathBuf::from("sweep/src/alphas"),
        };
    }

    let mut outputs = Vec::new();
    let mut bind = "127.0.0.1:8787".to_string();
    let mut data_dir = PathBuf::from("data");
    let mut args = raw.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--outputs" => {
                let spec = args.next().ok_or("--outputs requires a value")?;
                outputs.extend(parse_output_spec(&spec));
            }
            "--bind" => bind = args.next().ok_or("--bind requires an address")?,
            "--data-dir" => data_dir = args.next().ok_or("--data-dir requires a path")?.into(),
            "--execs-dir" => match &mut mode {
                Mode::Develop { execs_dir, .. } => {
                    *execs_dir = args.next().ok_or("--execs-dir requires a path")?.into();
                }
                Mode::View => return Err("--execs-dir is only valid in develop mode".into()),
            },
            "--help" | "-h" => {
                println!(
                    "usage:\n  execviz view --outputs [output1,output2/alpha/hash] [--bind 127.0.0.1:8787]\n  execviz develop <alpha_name> [--execs-dir sweep/src/alphas] [--bind 127.0.0.1:8787]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if outputs.is_empty() {
        outputs.push("output".into());
    }
    Ok(Args {
        mode,
        outputs,
        bind,
        data_dir,
    })
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    match args.mode {
        Mode::View => {
            let report = process::output_process(&args.outputs)?;
            let html = HtmlRenderer.render(&report)?;
            server::serve(&args.bind, report, html, args.data_dir, args.outputs)
        }
        Mode::Develop { alpha, execs_dir } => {
            let workspace = env::current_dir().map_err(|error| error.to_string())?;
            let execs_dir = if execs_dir.is_absolute() {
                execs_dir
            } else {
                workspace.join(execs_dir)
            };
            let (alpha_dir, created) = develop::prepare_alpha(&execs_dir, &alpha)?;
            if created {
                println!("created execution alpha:");
                println!("  {}", alpha_dir.join(format!("{alpha}.rs")).display());
                println!("  {}", alpha_dir.join(format!("{alpha}.json")).display());
            }
            fs::create_dir_all(workspace.join(".execlab/runs"))
                .map_err(|error| format!("cannot create run directory: {error}"))?;
            let config_path = alpha_dir.join(format!("{alpha}.json"));
            let alpha_config: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(&config_path)
                    .map_err(|error| format!("cannot read {}: {error}", config_path.display()))?,
            )
            .map_err(|error| format!("cannot parse {}: {error}", config_path.display()))?;
            let resolve = |field: &str| -> Result<PathBuf, String> {
                let value = alpha_config
                    .get(field)
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        format!("{} is missing string `{field}`", config_path.display())
                    })?;
                let path = PathBuf::from(value);
                Ok(if path.is_absolute() {
                    path
                } else {
                    workspace.join(path)
                })
            };
            let output_dir = resolve("output_dir")?;
            let data_dir = resolve("data_dir")?;
            let empty_report = process::Report {
                strategies: Vec::new(),
                warnings: Vec::new(),
            };
            let view_html = HtmlRenderer.render(&empty_report)?;
            let view = server::EmbeddedView::new(
                vec![output_dir.to_string_lossy().into_owned()],
                data_dir,
                view_html,
            );
            develop::serve(
                &args.bind,
                develop::DevelopConfig {
                    alpha,
                    alpha_dir,
                    workspace,
                },
                view,
            )
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("execviz: {error}");
        std::process::exit(1);
    }
}
