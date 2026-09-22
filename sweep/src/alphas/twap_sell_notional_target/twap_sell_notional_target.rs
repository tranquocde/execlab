//! Notional-target TWAP. The arrival mid determines the signed share obligation.

use execlab_core::{ExecutionSide, ExecutionTarget, TargetMode};
use hftbacktest::prelude::*;
use serde::{Deserialize, Serialize};

use crate::alpha::Alpha;

pub struct A;

const SECOND_NS: i64 = 1_000_000_000;
const DAY_NS: i64 = 86_400 * SECOND_NS;
const ICT_OFFSET_NS: i64 = 7 * 3_600 * SECOND_NS;
const SAMPLE_NS: i64 = 10 * SECOND_NS;
const BOARD_LOT: f64 = 100.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Params {
    pub start_time_seconds: i64,
    pub time_taken_seconds: f64,
    pub trade_frequency_seconds: f64,
    pub target_notional: f64,
    pub side: ExecutionSide,
}

fn ns(seconds: f64) -> Option<i64> {
    (seconds.is_finite() && seconds > 0.0).then(|| (seconds * SECOND_NS as f64).round() as i64)
}

fn start_ts(now: i64, seconds: i64) -> Option<i64> {
    if !(0..86_400).contains(&seconds) { return None; }
    Some((now + ICT_OFFSET_NS).div_euclid(DAY_NS) * DAY_NS - ICT_OFFSET_NS + seconds * SECOND_NS)
}

fn advance<MD, B>(hbt: &mut B, target: i64) -> bool
where MD: MarketDepth, B: Bot<MD>, B::Error: std::fmt::Debug {
    while hbt.current_timestamp() < target {
        let step = (target - hbt.current_timestamp()).min(SAMPLE_NS);
        if hbt.elapse(step).unwrap() == ElapseResult::EndOfData { return false; }
    }
    true
}

fn arrival<MD, B>(hbt: &mut B, p: &Params) -> Option<f64>
where MD: MarketDepth, B: Bot<MD>, B::Error: std::fmt::Debug {
    if hbt.elapse(1).unwrap() == ElapseResult::EndOfData { return None; }
    let start = start_ts(hbt.current_timestamp(), p.start_time_seconds)?;
    if !advance::<MD, B>(hbt, start) { return None; }
    loop {
        let depth = hbt.depth(0);
        let (bid, ask) = (depth.best_bid(), depth.best_ask());
        if bid.is_finite() && ask.is_finite() && bid > 0.0 && ask > bid {
            return Some((bid + ask) / 2.0);
        }
        if hbt.elapse(SAMPLE_NS).unwrap() == ElapseResult::EndOfData {
            return None;
        }
    }
}

fn signed_target(target: f64, price: f64, side: ExecutionSide) -> Option<f64> {
    if !target.is_finite() || target <= 0.0 || !price.is_finite() || price <= 0.0 { return None; }
    let qty = (target / price / BOARD_LOT).floor() * BOARD_LOT;
    (qty > 0.0).then_some(match side { ExecutionSide::Buy => -qty, ExecutionSide::Sell => qty })
}

fn drain<MD, B>(hbt: &mut B)
where MD: MarketDepth, B: Bot<MD>, B::Error: std::fmt::Debug {
    while hbt.elapse(SAMPLE_NS).unwrap() != ElapseResult::EndOfData {}
}

impl Alpha for A {
    type Params = Params;

    fn search_space() -> Vec<Params> {
        vec![Params { start_time_seconds: 36_000, time_taken_seconds: 1_800.0,
            trade_frequency_seconds: 60.0, target_notional: 100_000_000.0,
            side: ExecutionSide::Sell }]
    }

    fn execution_target(p: &Params) -> Option<ExecutionTarget> {
        Some(ExecutionTarget { mode: TargetMode::Notional, side: p.side, notional: Some(p.target_notional) })
    }

    fn has_data_dependent_initial_position() -> bool { true }

    fn resolve_initial_position<MD, B>(hbt: &mut B, p: &Params) -> Option<f64>
    where MD: MarketDepth, B: Bot<MD>, B::Error: std::fmt::Debug {
        signed_target(p.target_notional, arrival::<MD, B>(hbt, p)?, p.side)
    }

    fn run<MD, B>(hbt: &mut B, p: &Params)
    where MD: MarketDepth, B: Bot<MD>, B::Error: std::fmt::Debug {
        let Some(window) = ns(p.time_taken_seconds) else { hbt.close().unwrap(); return; };
        let Some(frequency) = ns(p.trade_frequency_seconds) else { hbt.close().unwrap(); return; };
        let Some(arrival) = arrival::<MD, B>(hbt, p) else { drain::<MD, B>(hbt); hbt.close().unwrap(); return; };
        let start = hbt.current_timestamp();
        let Some(end) = start.checked_add(window) else { drain::<MD, B>(hbt); hbt.close().unwrap(); return; };
        let slices = ((window + frequency - 1) / frequency).max(1) as f64;
        let slice_value = p.target_notional / slices;
        let fixed_qty = (slice_value / arrival / BOARD_LOT).ceil() * BOARD_LOT;
        let mut next = start;
        let mut order_id = 0;
        while next < end && fixed_qty > 0.0 {
            if !advance::<MD, B>(hbt, next) { hbt.close().unwrap(); return; }
            let executed = hbt.state_values(0).trading_value;
            let remaining = p.target_notional - executed;
            let depth = hbt.depth(0);
            let (bid, ask) = (depth.best_bid(), depth.best_ask());
            if remaining > 0.0 && bid.is_finite() && ask.is_finite() && ask > bid {
                let executable = match p.side { ExecutionSide::Buy => ask, ExecutionSide::Sell => bid };
                let cap = (remaining / executable / BOARD_LOT).floor() * BOARD_LOT;
                let qty = fixed_qty.min(cap).min(hbt.position(0).abs());
                if qty < BOARD_LOT { break; }
                hbt.clear_inactive_orders(Some(0));
                order_id += 1;
                let mid = (bid + ask) / 2.0;
                match p.side {
                    ExecutionSide::Buy => { let _ = hbt.submit_buy_order(0, order_id, mid, qty, TimeInForce::GTC, OrdType::Market, true); }
                    ExecutionSide::Sell => { let _ = hbt.submit_sell_order(0, order_id, mid, qty, TimeInForce::GTC, OrdType::Market, true); }
                }
            }
            let Some(value) = next.checked_add(frequency) else { break; };
            next = value;
        }
        drain::<MD, B>(hbt);
        hbt.close().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signs_and_rounds_target() {
        assert_eq!(signed_target(105_000.0, 1_000.0, ExecutionSide::Sell), Some(100.0));
        assert_eq!(signed_target(105_000.0, 1_000.0, ExecutionSide::Buy), Some(-100.0));
    }
}
