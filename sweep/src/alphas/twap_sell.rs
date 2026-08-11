//! One alpha = one file: params, search space, and logic. Nothing else in the
//! crate changes when you add another.

use hftbacktest::prelude::*;
use serde::{Deserialize, Serialize};

use crate::alpha::Alpha;

/// The type name build.rs looks for. Always `A` — the FILE is the identity.
pub struct A;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Params {
    /// i64 because that's what `Bot::elapse` takes (types.rs:934).
    pub elapse_ns: i64,
    pub slice_qty: f64,
}

impl Alpha for A {
    type Params = Params;

    // ---- the search space, owned by the alpha ----
    fn search_space() -> Vec<Params> {
        let mut v = Vec::new();
        for elapse_ns in (1..=4).map(|i| i * 10_000_000i64) {
            for slice_qty in [2.0, 10.0, 20.0] {
                v.push(Params {
                    elapse_ns,
                    slice_qty,
                });
            }
        }
        v // 20 combinations
    }

    // ---- the trading logic ----
    // DRAFT: illustrative only — the point is the shape, not the edge.
    fn run<MD, B>(hbt: &mut B, p: &Params)
    where
        MD: MarketDepth,
        B: Bot<MD>,
        B::Error: std::fmt::Debug,
    {
        let mut order_id = 0;

        while hbt.elapse(p.elapse_ns).unwrap() == ElapseResult::Ok {
            let depth = hbt.depth(0);
            let (bb, ba) = (depth.best_bid(), depth.best_ask());
            if !(bb > 0.0 && ba > 0.0) {
                continue;
            }
            let pos = hbt.position(0);
            if pos <= 0.0 {
                break;
            }

            hbt.clear_inactive_orders(Some(0));
            order_id += 1;
            let _ = hbt.submit_sell_order(
                0,
                order_id,
                (bb + ba) / 2.0,
                p.slice_qty,
                TimeInForce::GTC,
                OrdType::Market,
                true,
            );
        }

        hbt.close().unwrap();
    }
}
