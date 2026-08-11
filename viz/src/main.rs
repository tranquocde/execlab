mod process;
mod replay_view;
mod render;
mod server;

use std::{env, path::PathBuf};

use render::{html::HtmlRenderer, Renderer};

struct Args {
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
    let mut outputs = Vec::new();
    let mut bind = "127.0.0.1:8787".to_string();
    let mut data_dir = PathBuf::from("data");
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--outputs" => {
                let spec = args.next().ok_or("--outputs requires a value")?;
                outputs.extend(parse_output_spec(&spec));
            }
            "--bind" => bind = args.next().ok_or("--bind requires an address")?,
            "--data-dir" => data_dir = args.next().ok_or("--data-dir requires a path")?.into(),
            "--help" | "-h" => {
                println!(
                    "usage: execviz --outputs [output1,output2/alpha,output3/alpha/hash] \\\n+                     [--bind 127.0.0.1:8787]"
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
        outputs,
        bind,
        data_dir,
    })
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let report = process::output_process(&args.outputs)?;
    let html = HtmlRenderer.render(&report)?;
    server::serve(&args.bind, report, html, args.data_dir, args.outputs)
}

fn main() {
    if let Err(error) = run() {
        eprintln!("execviz: {error}");
        std::process::exit(1);
    }
}
