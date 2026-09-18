//! Lightweight sweep-only `Bot` wrapper.
//!
//! Counts submissions without scanning the order map or retaining replay data.
//! `n_maker` follows the dashboard convention: submitted limit orders.

use std::collections::HashMap;

use hftbacktest::{
    backtest::Backtest,
    prelude::{Bot, HashMapMarketDepth, MarketDepth},
    types::{ElapseResult, Event, OrdType, Order, OrderId, OrderRequest, StateValues, TimeInForce},
};

trait EffectiveDepthAccess {
    fn effective_depth(&self, asset_no: usize) -> Option<&HashMapMarketDepth>;
}

impl<MD: MarketDepth> EffectiveDepthAccess for Backtest<MD> {
    fn effective_depth(&self, asset_no: usize) -> Option<&HashMapMarketDepth> {
        Backtest::effective_depth(self, asset_no)
    }
}

pub struct SweepObserved<B> {
    inner: B,
    num_orders: usize,
    n_maker: usize,
    session_start_ts: Option<i64>,
    arrival_mid_price: Option<f64>,
    arrival_capture_attempted: bool,
    divergence_sum: f64,
    divergence_count: usize,
    max_divergence_score: Option<f64>,
}

impl<B> SweepObserved<B> {
    pub fn new(inner: B) -> Self {
        Self {
            inner,
            num_orders: 0,
            n_maker: 0,
            session_start_ts: None,
            arrival_mid_price: None,
            arrival_capture_attempted: false,
            divergence_sum: 0.0,
            divergence_count: 0,
            max_divergence_score: None,
        }
    }

    pub fn num_orders(&self) -> usize {
        self.num_orders
    }
    pub fn n_maker(&self) -> usize {
        self.n_maker
    }
    pub fn session_start_ts(&self) -> Option<i64> {
        self.session_start_ts
    }
    pub fn arrival_mid_price(&self) -> Option<f64> {
        self.arrival_mid_price
    }
    pub fn mean_divergence_score(&self) -> Option<f64> {
        (self.divergence_count > 0).then_some(self.divergence_sum / self.divergence_count as f64)
    }
    pub fn max_divergence_score(&self) -> Option<f64> {
        self.max_divergence_score
    }

    fn capture_arrival<MD>(&mut self, asset_no: usize)
    where
        MD: MarketDepth,
        B: Bot<MD>,
    {
        if self.arrival_capture_attempted {
            return;
        }
        self.arrival_capture_attempted = true;
        let depth = self.inner.depth(asset_no);
        let (bid, ask) = (depth.best_bid(), depth.best_ask());
        self.arrival_mid_price = match (bid.is_finite(), ask.is_finite()) {
            (true, true) => Some((bid + ask) / 2.0),
            (true, false) => Some(bid),
            (false, true) => Some(ask),
            (false, false) => None,
        };
    }

    fn count_submission(&mut self, order_type: OrdType) {
        self.num_orders += 1;
        if order_type == OrdType::Limit {
            self.n_maker += 1;
        }
    }

    fn sample_divergence<MD>(&mut self)
    where
        MD: MarketDepth,
        B: Bot<MD> + EffectiveDepthAccess,
    {
        let depth = self.inner.depth(0);
        let (bid, ask) = (depth.best_bid(), depth.best_ask());
        let Some(effective) = self.inner.effective_depth(0) else {
            return;
        };
        let (effective_bid, effective_ask) = (effective.best_bid(), effective.best_ask());
        let normal_spread = ask - bid;
        let effective_spread = effective_ask - effective_bid;
        if !bid.is_finite()
            || !ask.is_finite()
            || !effective_bid.is_finite()
            || !effective_ask.is_finite()
            || !normal_spread.is_finite()
            || normal_spread <= f64::EPSILON
            || !effective_spread.is_finite()
        {
            return;
        }
        let score = effective_spread / normal_spread;
        if !score.is_finite() {
            return;
        }
        self.divergence_sum += score;
        self.divergence_count += 1;
        self.max_divergence_score = Some(
            self.max_divergence_score
                .map_or(score, |current| current.max(score)),
        );
    }
}

impl<MD, B> Bot<MD> for SweepObserved<B>
where
    MD: MarketDepth,
    B: Bot<MD> + EffectiveDepthAccess,
{
    type Error = B::Error;

    fn current_timestamp(&self) -> i64 {
        self.inner.current_timestamp()
    }
    fn num_assets(&self) -> usize {
        self.inner.num_assets()
    }
    fn position(&self, asset_no: usize) -> f64 {
        self.inner.position(asset_no)
    }
    fn state_values(&self, asset_no: usize) -> &StateValues {
        self.inner.state_values(asset_no)
    }
    fn depth(&self, asset_no: usize) -> &MD {
        self.inner.depth(asset_no)
    }
    fn last_trades(&self, asset_no: usize) -> &[Event] {
        self.inner.last_trades(asset_no)
    }
    fn clear_last_trades(&mut self, asset_no: Option<usize>) {
        self.inner.clear_last_trades(asset_no)
    }
    fn orders(&self, asset_no: usize) -> &HashMap<OrderId, Order> {
        self.inner.orders(asset_no)
    }

    #[allow(clippy::too_many_arguments)]
    fn submit_buy_order(
        &mut self,
        asset_no: usize,
        order_id: OrderId,
        price: f64,
        qty: f64,
        time_in_force: TimeInForce,
        order_type: OrdType,
        wait: bool,
    ) -> Result<ElapseResult, Self::Error> {
        self.capture_arrival::<MD>(asset_no);
        self.count_submission(order_type);
        self.inner.submit_buy_order(
            asset_no,
            order_id,
            price,
            qty,
            time_in_force,
            order_type,
            wait,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn submit_sell_order(
        &mut self,
        asset_no: usize,
        order_id: OrderId,
        price: f64,
        qty: f64,
        time_in_force: TimeInForce,
        order_type: OrdType,
        wait: bool,
    ) -> Result<ElapseResult, Self::Error> {
        self.capture_arrival::<MD>(asset_no);
        self.count_submission(order_type);
        self.inner.submit_sell_order(
            asset_no,
            order_id,
            price,
            qty,
            time_in_force,
            order_type,
            wait,
        )
    }

    fn submit_order(
        &mut self,
        asset_no: usize,
        order: OrderRequest,
        wait: bool,
    ) -> Result<ElapseResult, Self::Error> {
        self.capture_arrival::<MD>(asset_no);
        self.count_submission(order.order_type);
        self.inner.submit_order(asset_no, order, wait)
    }

    fn modify(
        &mut self,
        asset_no: usize,
        order_id: OrderId,
        price: f64,
        qty: f64,
        wait: bool,
    ) -> Result<ElapseResult, Self::Error> {
        self.inner.modify(asset_no, order_id, price, qty, wait)
    }

    fn cancel(
        &mut self,
        asset_no: usize,
        order_id: OrderId,
        wait: bool,
    ) -> Result<ElapseResult, Self::Error> {
        self.inner.cancel(asset_no, order_id, wait)
    }

    fn clear_inactive_orders(&mut self, asset_no: Option<usize>) {
        self.inner.clear_inactive_orders(asset_no)
    }

    fn wait_order_response(
        &mut self,
        asset_no: usize,
        order_id: OrderId,
        timeout: i64,
    ) -> Result<ElapseResult, Self::Error> {
        self.inner.wait_order_response(asset_no, order_id, timeout)
    }

    fn wait_next_feed(
        &mut self,
        include_order_resp: bool,
        timeout: i64,
    ) -> Result<ElapseResult, Self::Error> {
        self.inner.wait_next_feed(include_order_resp, timeout)
    }

    fn elapse(&mut self, duration: i64) -> Result<ElapseResult, Self::Error> {
        let result = self.inner.elapse(duration)?;
        if self.session_start_ts.is_none() {
            self.session_start_ts = Some(self.inner.current_timestamp());
        }
        self.sample_divergence::<MD>();
        Ok(result)
    }

    fn elapse_bt(&mut self, duration: i64) -> Result<ElapseResult, Self::Error> {
        let result = self.inner.elapse_bt(duration)?;
        if self.session_start_ts.is_none() {
            self.session_start_ts = Some(self.inner.current_timestamp());
        }
        self.sample_divergence::<MD>();
        Ok(result)
    }

    fn close(&mut self) -> Result<(), Self::Error> {
        self.inner.close()
    }

    fn feed_latency(&self, asset_no: usize) -> Option<(i64, i64)> {
        self.inner.feed_latency(asset_no)
    }

    fn order_latency(&self, asset_no: usize) -> Option<(i64, i64, i64)> {
        self.inner.order_latency(asset_no)
    }
}
