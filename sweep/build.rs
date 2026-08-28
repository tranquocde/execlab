//! Auto-discovers alphas at build time.
//!
//! Every `src/alphas/<name>.rs` that defines `pub struct A` implementing `Alpha`
//! is picked up automatically — no `mod` line to add, no registry to edit.
//! Drop the file in, `cargo run --release -- <name>` works.
//!
//! We also hash each alpha's *normalized* source here (comments and blank lines
//! stripped) so `code_hash` changes when the LOGIC changes but not when you
//! reword a comment. That hash goes into the output directory name, so it must
//! stay stable across cosmetic edits — otherwise every trivial edit orphans the
//! entire results tree.

use std::{env, fs, path::Path};

fn normalize(src: &str) -> String {
    src.lines()
        .map(|l| l.split("//").next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Files OUTSIDE the alpha that still change the numbers.
///
/// `engine.rs` sets fees, latency, queue model, exchange kind, tick/lot size.
/// `row.rs` computes pnl and settle. Editing either moves every result, so both
/// must be part of the identity — otherwise old directories keep their names,
/// keep their completion markers, and silently mix results computed by two
/// different formulas. That failure is invisible, which makes it the worst kind.
///
/// Cost: touching `row.rs` invalidates EVERY alpha. That is correct — the
/// numbers really did all change.
const RESULT_AFFECTING: &[&str] = &[
    "src/engine.rs",         // fees, latency, queue model, exchange kind, tick/lot
    "src/extract.rs",        // pnl and settle
    "src/sweep_observer.rs", // sweep activity counters
    "../core/src/row.rs",    // the row schema itself
];

fn main() {
    println!("cargo:rerun-if-changed=src/alphas");

    let mut shared = blake3::Hasher::new();
    for path in RESULT_AFFECTING {
        println!("cargo:rerun-if-changed={path}");
        let src = fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        shared.update(normalize(&src).as_bytes());
    }
    let shared = shared.finalize();

    let mut found: Vec<(String, String, String, String)> = fs::read_dir("src/alphas")
        .expect("src/alphas missing")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let (name, source, config) = if path.is_dir() {
                let name = path.file_name()?.to_str()?.to_string();
                (
                    name.clone(),
                    path.join(format!("{name}.rs")),
                    path.join(format!("{name}.json")),
                )
            } else {
                let stem = path.file_stem()?.to_str()?.to_string();
                if path.extension()? != "rs" || stem == "mod" {
                    return None;
                }
                (
                    stem.clone(),
                    path,
                    Path::new("src/alphas").join(format!("{stem}.json")),
                )
            };
            if !source.is_file() {
                panic!("alpha {name} is missing {}", source.display());
            }
            if !config.is_file() {
                panic!("alpha {name} is missing {}", config.display());
            }
            println!("cargo:rerun-if-changed={}", source.display());
            println!("cargo:rerun-if-changed={}", config.display());

            let src = fs::read_to_string(&source).ok()?;

            // alpha logic + shared result-affecting code
            let mut h = blake3::Hasher::new();
            h.update(normalize(&src).as_bytes());
            h.update(shared.as_bytes());

            Some((
                name,
                h.finalize().to_hex()[..8].to_string(),
                source.to_string_lossy().into_owned(),
                config.to_string_lossy().into_owned(),
            ))
        })
        .collect();
    found.sort();

    // `#[path]` with an ABSOLUTE path is required: the generated file is
    // include!d from src/alphas/mod.rs but module resolution is relative to the
    // generated file's own location in OUT_DIR, where the sources aren't.
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let mods = found
        .iter()
        .map(|(name, _, source, _)| {
            format!("#[path = \"{manifest_dir}/{source}\"]\npub mod {name};")
        })
        .collect::<Vec<_>>()
        .join("\n");

    let entries = found
        .iter()
        .map(|(name, hash, _, config)| format!(
            "        crate::entry::<{name}::A>(\"{name}\", \"{hash}\", include_str!(\"{manifest_dir}/{config}\")),"
        ))
        .collect::<Vec<_>>()
        .join("\n");

    // Each alpha's source, embedded verbatim, so `strategy.rs` can carry the
    // actual logic rather than a promise that it exists elsewhere. include_str!
    // guarantees byte-identical text — no Rust parsing, nothing to drift.
    let sources = found
        .iter()
        .map(|(name, _, source, _)| {
            format!("        \"{name}\" => include_str!(\"{manifest_dir}/{source}\"),")
        })
        .collect::<Vec<_>>()
        .join("\n");

    let generated = format!(
        "// @generated by build.rs — do not edit\n\
         {mods}\n\n\
         pub fn registry() -> Vec<crate::Entry> {{\n    vec![\n{entries}\n    ]\n}}\n\n\
         /// Verbatim source of an alpha, for the deploy/record artifact.\n\
         pub fn alpha_source(name: &str) -> &'static str {{\n    match name {{\n\
         {sources}\n        _ => \"\",\n    }}\n}}\n\n\
         /// Engine configuration is part of \"what this alpha does\" — fees,\n\
         /// tick/lot size and exchange kind change behaviour, so the record\n\
         /// carries them too.\n\
         pub const ENGINE_SOURCE: &str = include_str!(\"{manifest_dir}/src/engine.rs\");\n"
    );

    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("alphas_gen.rs"),
        generated,
    )
    .unwrap();
}
