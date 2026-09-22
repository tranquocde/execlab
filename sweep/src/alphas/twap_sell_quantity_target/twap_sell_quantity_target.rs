//! Quantity-target TWAP for full-day market-data sessions.

use hftbacktest::prelude::*;
use serde::{Deserialize, Serialize};
use execlab_core::{ExecutionSide, ExecutionTarget, TargetMode};

use crate::alpha::Alpha;

pub struct A;

const SECOND_NS: i64 = 1_000_000_000;
const HOUR_NS: i64 = 3_600 * SECOND_NS;
const DAY_NS: i64 = 24 * HOUR_NS;
const ICT_OFFSET_NS: i64 = 7 * HOUR_NS;
const SAMPLE_NS: i64 = 10 * SECOND_NS;
const ROUND_LOT: f64 = 100.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Params {
    /// Seconds after midnight in ICT.
    pub start_time_seconds: i64,
    pub time_taken_seconds: f64,
    pub trade_frequency_seconds: f64,
    #[serde(default)]
    pub target_mode: TargetMode,
    #[serde(default)]
    pub target_notional: Option<f64>,
    #[serde(default)]
    pub side: Option<ExecutionSide>,
}

fn seconds_to_ns(seconds: f64) -> Option<i64> {
    (seconds.is_finite() && seconds > 0.0 && seconds <= i64::MAX as f64 / SECOND_NS as f64)
        .then(|| (seconds * SECOND_NS as f64).round() as i64)
}

fn execution_start(timestamp: i64, start_time_seconds: i64) -> Option<i64> {
    if !(0..86_400).contains(&start_time_seconds) {
        return None;
    }
    let ict_day = (timestamp + ICT_OFFSET_NS).div_euclid(DAY_NS);
    Some(ict_day * DAY_NS - ICT_OFFSET_NS + start_time_seconds * SECOND_NS)
}

fn slice_quantity(target: f64, time_taken_ns: i64, frequency_ns: i64) -> Option<f64> {
    if !target.is_finite() || target <= 0.0 || time_taken_ns <= 0 || frequency_ns <= 0 {
        return None;
    }
    let slices = time_taken_ns.checked_add(frequency_ns - 1)? / frequency_ns;
    let slices = slices.max(1) as f64;
    Some(((target / slices) / ROUND_LOT).ceil() * ROUND_LOT)
}

fn submission_count(time_taken_ns: i64, frequency_ns: i64) -> Option<i64> {
    (time_taken_ns > 0 && frequency_ns > 0)
        .then(|| time_taken_ns.checked_add(frequency_ns - 1))
        .flatten()
        .map(|n| (n / frequency_ns).max(1))
}

fn notional_slice_quantity(target: f64, executed: f64, price: f64, slices: i64) -> Option<f64> {
    if !target.is_finite() || target <= 0.0 || !executed.is_finite() || !price.is_finite()
        || price <= 0.0 || slices <= 0
    {
        return None;
    }
    let remaining = target - executed;
    if remaining <= 0.0 {
        return None;
    }
    let slice_value = (target / slices as f64).min(remaining);
    Some((slice_value / price / ROUND_LOT).ceil() * ROUND_LOT)
}

fn advance_to<MD, B>(hbt: &mut B, target_ts: i64) -> bool
where
    MD: MarketDepth,
    B: Bot<MD>,
    B::Error: std::fmt::Debug,
{
    while hbt.current_timestamp() < target_ts {
        let elapsed = (target_ts - hbt.current_timestamp()).min(SAMPLE_NS);
        if hbt.elapse(elapsed).unwrap() == ElapseResult::EndOfData {
            return false;
        }
    }
    true
}

fn drain<MD, B>(hbt: &mut B)
where
    MD: MarketDepth,
    B: Bot<MD>,
    B::Error: std::fmt::Debug,
{
    while hbt.elapse(SAMPLE_NS).unwrap() != ElapseResult::EndOfData {}
}

impl Alpha for A {
    type Params = Params;

    fn execution_target(p: &Params) -> Option<ExecutionTarget> {
        (p.target_mode == TargetMode::Notional).then_some(ExecutionTarget {
            mode: TargetMode::Notional,
            side: p.side?,
            notional: p.target_notional,
        })
    }

    fn search_space() -> Vec<Params> {
        let mut params = Vec::new();
        for start_time_seconds in [10 * 3_600] {
            for time_taken_seconds in [1_800.0, 3_600.0] {
                for trade_frequency_seconds in [30.0, 60.0] {
                    params.push(Params {
                        start_time_seconds,
                        time_taken_seconds,
                        trade_frequency_seconds,
                        target_mode: TargetMode::Quantity,
                        target_notional: None,
                        side: None,
                    });
                }
            }
        }
        params
    }

    fn run<MD, B>(hbt: &mut B, p: &Params)
    where
        MD: MarketDepth,
        B: Bot<MD>,
        B::Error: std::fmt::Debug,
    {
        let Some(time_taken_ns) = seconds_to_ns(p.time_taken_seconds) else {
            hbt.close().unwrap();
            return;
        };
        let Some(frequency_ns) = seconds_to_ns(p.trade_frequency_seconds) else {
            hbt.close().unwrap();
            return;
        };

        // Initialize the engine at the first feed event before resolving the
        // requested ICT wall-clock time for this session's date.
        if hbt.elapse(1).unwrap() == ElapseResult::EndOfData {
            hbt.close().unwrap();
            return;
        }
        let Some(start_ts) = execution_start(hbt.current_timestamp(), p.start_time_seconds) else {
            drain::<MD, B>(hbt);
            hbt.close().unwrap();
            return;
        };
        let Some(end_ts) = start_ts.checked_add(time_taken_ns) else {
            drain::<MD, B>(hbt);
            hbt.close().unwrap();
            return;
        };

        if !advance_to::<MD, B>(hbt, start_ts) {
            hbt.close().unwrap();
            return;
        }

        let quantity_slice = match p.target_mode {
            TargetMode::Quantity => slice_quantity(
                hbt.position(0).abs(),
                time_taken_ns,
                frequency_ns,
            ),
            TargetMode::Notional => None,
        };
        let slices = submission_count(time_taken_ns, frequency_ns).unwrap_or(1);

        let mut order_id = 0;
        let mut next_trade_ts = start_ts.max(hbt.current_timestamp());
        while next_trade_ts < end_ts {
            if !advance_to::<MD, B>(hbt, next_trade_ts) {
                hbt.close().unwrap();
                return;
            }

            let depth = hbt.depth(0);
            let (best_bid, best_ask) = (depth.best_bid(), depth.best_ask());
            if best_bid > 0.0 && best_ask > best_bid {
                let position = hbt.position(0);
                let price = (best_bid + best_ask) / 2.0;
                let (side, qty) = match p.target_mode {
                    TargetMode::Quantity => {
                        if position.abs() <= f64::EPSILON {
                            break;
                        }
                        let Some(slice_qty) = quantity_slice else { break };
                        let side = if position < 0.0 {
                            ExecutionSide::Buy
                        } else {
                            ExecutionSide::Sell
                        };
                        (side, slice_qty.min(position.abs()))
                    }
                    TargetMode::Notional => {
                        let Some(target) = p.target_notional else { break };
                        let executed = hbt.state_values(0).trading_value;
                        let Some(qty) = notional_slice_quantity(target, executed, price, slices)
                        else {
                            break;
                        };
                        let Some(side) = p.side else { break };
                        (side, qty)
                    }
                };
                hbt.clear_inactive_orders(Some(0));
                order_id += 1;
                if side == ExecutionSide::Buy {
                    let _ = hbt.submit_buy_order(
                        0,
                        order_id,
                        price,
                        qty,
                        TimeInForce::GTC,
                        OrdType::Market,
                        true,
                    );
                } else {
                    let _ = hbt.submit_sell_order(
                        0,
                        order_id,
                        price,
                        qty,
                        TimeInForce::GTC,
                        OrdType::Market,
                        true,
                    );
                }
            }

            let Some(next) = next_trade_ts.checked_add(frequency_ns) else {
                break;
            };
            next_trade_ts = next;
        }

        // Continue to the end of the data without submitting new orders so
        // final-mid valuation still represents the end of the full session.
        drain::<MD, B>(hbt);
        hbt.close().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_slice_up_to_board_lot() {
        assert_eq!(slice_quantity(10_500.0, 600, 60), Some(1_100.0));
        assert_eq!(slice_quantity(1_000.0, 60, 20), Some(400.0));
    }

    #[test]
    fn partial_window_adds_a_submission_opportunity() {
        assert_eq!(slice_quantity(1_000.0, 61, 30), Some(400.0));
    }

    #[test]
    fn notional_slice_rounds_up_to_board_lot_and_caps_value_before_rounding() {
        assert_eq!(notional_slice_quantity(1_000_000.0, 0.0, 20_000.0, 10), Some(100.0));
        assert_eq!(notional_slice_quantity(1_000_000.0, 950_000.0, 20_000.0, 10), Some(100.0));
        assert_eq!(notional_slice_quantity(1_000_000.0, 1_000_000.0, 20_000.0, 10), None);
    }
}
