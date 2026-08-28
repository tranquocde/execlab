use hftbacktest::prelude::*;
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
