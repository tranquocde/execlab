//! Emits `strategy.rs`: the full, readable record of one combination.
//!
//! Goal: someone opening this file — you in six months, or a reviewer — sees
//! the ENTIRE idea without opening the repo. That needs three things, and the
//! artifact is incomplete without any of them:
//!
//!   1. provenance  — which code, which data, is it still reproducible
//!   2. the params  — frozen, no search space
//!   3. the logic   — verbatim: the alpha AND the engine config it assumes
//!
//! Params are emitted as a JSON string rather than a generated Rust literal.
//! Turning `Quoting::InventorySkewed { gamma: 0.03 }` back into Rust source
//! needs type-aware codegen that breaks on every new param shape; a JSON string
//! plus `serde_json::from_str` is general and works for any alpha.

use execlab_core::output::DataRef;
use serde::Serialize;

pub fn render<P: Serialize>(
    alpha: &str,
    hash: &str,
    code_hash: &str,
    params: &P,
    data: &DataRef,
) -> String {
    let params_json = serde_json::to_string(params).unwrap();
    let params_pretty = serde_json::to_string_pretty(params)
        .unwrap()
        .lines()
        .map(|l| format!("//   {l}"))
        .collect::<Vec<_>>()
        .join("\n");

    let source = crate::alphas::alpha_source(alpha);
    let engine = crate::alphas::ENGINE_SOURCE;

    format!(
        r###"// ============================================================================
// {alpha} · {hash}
//
// A FIXED RULE. Params are frozen below; there is no search space here.
//
//   code_hash   {code_hash}   (alpha logic + engine.rs + extract.rs, normalized)
//   data        {dir} · {n} sessions
//   universe    {universe}
//
// Reproduce:  execlab --data-dir {dir} {alpha}
// Stats:      manifest.json beside this file — single source of truth,
//             deliberately not duplicated here so the two cannot disagree.
//
// PARAMS
{params_pretty}
// ============================================================================

/// Frozen parameters for this combination. `Params` is defined in the verbatim
/// alpha source below, so this file describes itself completely.
pub const PARAMS_JSON: &str = r##"{params_json}"##;

// ----------------------------------------------------------------------------
// THE LOGIC - verbatim copy of src/alphas/{alpha}.rs
//
// Byte-identical to what produced the results, guaranteed by include_str! at
// build time rather than by extraction (which would be fragile). `run()` is
// generic over `Bot`, so the same code runs against a live bot unchanged.
// `search_space()` rides along unused - the params above are what was run.
// ----------------------------------------------------------------------------
/*
{source}
*/

// ----------------------------------------------------------------------------
// THE ENGINE CONFIG IT ASSUMES - verbatim copy of src/engine.rs
//
// Not incidental: tick/lot size, fee model, latency, queue model and exchange
// kind all change what the logic above does. Identical alpha source under two
// engine configs is two different strategies - which is why this file is part
// of `code_hash`.
// ----------------------------------------------------------------------------
/*
{engine}
*/
"###,
        dir = data.dir,
        n = data.n_sessions,
        universe = data.file_list_hash,
    )
}
