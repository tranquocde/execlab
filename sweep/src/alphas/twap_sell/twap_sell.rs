//! One alpha folder: this file owns params, search space, and trading logic.
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
        [
            (10, 100.0),
            (20, 500.0),
            (300, 1_000.0),
            (400 , 2_000.0)
        ]
        .into_iter()
        .map(|(elapse_units, slice_qty)| Params {
            elapse_ns: elapse_units * 100_000_000i64,
            slice_qty,
        })
        .collect() // 3 paired combinations
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
            if !(bb > 0.0 && ba > bb) {
                continue;
            }
            let pos = hbt.position(0);
            if pos.abs() <= f64::EPSILON {
                continue;
            }

            hbt.clear_inactive_orders(Some(0));
            order_id += 1;
            //if order_id % 10 !=0 {continue;}
            let qty = p.slice_qty.min(pos.abs());
            if pos < 0.0 {
                let _ = hbt.submit_buy_order(
                    0, order_id, (bb + ba) / 2.0, qty,
                    TimeInForce::GTC, OrdType::Market, true,
                );
            } else {
                let _ = hbt.submit_sell_order(
                    0, order_id, (bb + ba) / 2.0, qty,
                    TimeInForce::GTC, OrdType::Market, true,
                );
            }


        }

        hbt.close().unwrap();
    }
}
