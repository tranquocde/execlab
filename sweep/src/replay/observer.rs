//! `Observed<B>` — a `Bot` that wraps another `Bot`, delegating everything and
//! recording what happens on the way through.
//!
//! This exists because the engine exposes NO fill event stream. `orders()`
//! returns the current local view, so fills are detected by diffing each
//! order's cumulative executed quantity (`qty - leaves_qty`) between samples.
//!
//! TIMING IS LOAD-BEARING. Alphas call `clear_inactive_orders()` after
//! `elapse()` returns, which drops filled orders from the map. We therefore
//! sample INSIDE `elapse`, immediately after the inner call — before control
//! goes back to the alpha. Sample any later and the fills are already gone.
//!
//! The alpha cannot tell it is wrapped: `Alpha::run` is generic over `Bot`, so
//! this substitutes for `Backtest<..>` with no change to any alpha. Same
//! property that makes live deployment a swap rather than a rewrite.

use std::collections::HashMap;

use hftbacktest::{
    backtest::Backtest,
    prelude::{Bot, HashMapMarketDepth, MarketDepth},
    types::{ElapseResult, Event, OrdType, Order, OrderId, OrderRequest, StateValues, TimeInForce},
};

use super::record::{Fill, Sample};

trait EffectiveDepthAccess {
    fn effective_depth(&self, asset_no: usize) -> Option<&HashMapMarketDepth>;
}

impl<MD: MarketDepth> EffectiveDepthAccess for Backtest<MD> {
    fn effective_depth(&self, asset_no: usize) -> Option<&HashMapMarketDepth> {
        Backtest::effective_depth(self, asset_no)
    }
}

pub struct Observed<B> {
    inner: B,
    /// order_id -> cumulative executed quantity already recorded.
    ///
    /// `Order::exec_qty` is only the latest execution, not the cumulative
    /// quantity. The cumulative quantity is `qty - leaves_qty`. Order IDs must
    /// therefore be unique for the lifetime of a replay.
    seen: HashMap<OrderId, f64>,
    pub fills: Vec<Fill>,
    pub curve: Vec<Sample>,
    pub max_inventory: f64,
}

impl<B> Observed<B> {
    pub fn new(inner: B) -> Self {
        Self {
            inner,
            seen: HashMap::new(),
            fills: Vec::new(),
            curve: Vec::new(),
            max_inventory: 0.0,
        }
    }
}

/// NaN means "no such side" in this engine (hashmapmarketdepth.rs:255).
/// Carry that through as null rather than inventing a price.
fn opt(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}

impl<B> Observed<B> {
    fn sample<MD>(&mut self)
    where
        MD: MarketDepth,
        B: Bot<MD> + EffectiveDepthAccess,
    {
        let ts = self.inner.current_timestamp();
        let sv = self.inner.state_values(0);
        let (position, balance, fee) = (sv.position, sv.balance, sv.fee);

        // ---- fills: diff cumulative execution against what was recorded ----
        let mut new_fills = Vec::new();
        let mut my_bid: Option<f64> = None;
        let mut my_ask: Option<f64> = None;

        for (id, o) in self.inner.orders(0).iter() {
            let px = o.price_tick as f64 * o.tick_size;

            // Track where our live quotes sit (best of each side).
            if o.leaves_qty > 0.0 {
                let is_buy: f64 = *AsRef::<f64>::as_ref(&o.side);
                if is_buy > 0.0 {
                    my_bid = Some(my_bid.map_or(px, |b: f64| b.max(px)));
                } else if is_buy < 0.0 {
                    my_ask = Some(my_ask.map_or(px, |a: f64| a.min(px)));
                }
            }

            let total_executed = (o.qty - o.leaves_qty).max(0.0);
            let previously_recorded = self.seen.get(id).copied().unwrap_or(0.0);
            let delta = total_executed - previously_recorded;
            self.seen.insert(*id, total_executed);
            if delta > 1e-12 {
                new_fills.push(Fill {
                    ts: o.exch_timestamp,
                    ts_local: o.local_timestamp,
                    order_id: *id,
                    side: if *AsRef::<f64>::as_ref(&o.side) > 0.0 {
                        "buy"
                    } else {
                        "sell"
                    }
                    .into(),
                    px: o.exec_price_tick as f64 * o.tick_size,
                    qty: delta,
                    maker: o.maker,
                    position,
                    balance,
                    fee,
                });
            }
        }
        // Deterministic order: `orders()` is a HashMap, so iteration order is
        // not stable across runs. Without this the fill list could differ run
        // to run even though the numbers are identical.
        new_fills.sort_by_key(|f| (f.ts, f.order_id));
        self.fills.extend(new_fills);

        // ---- state snapshot ----
        let depth = self.inner.depth(0);
        let (bid, ask) = (opt(depth.best_bid()), opt(depth.best_ask()));
        let (effective_bid, effective_ask) = self
            .inner
            .effective_depth(0)
            .map(|depth| (opt(depth.best_bid()), opt(depth.best_ask())))
            .unwrap_or((None, None));
        let mid = match (bid, ask) {
            (Some(b), Some(a)) => Some((b + a) / 2.0),
            _ => None,
        };

        if position.abs() > self.max_inventory {
            self.max_inventory = position.abs();
        }

        self.curve.push(Sample {
            ts,
            bid,
            ask,
            effective_bid,
            effective_ask,
            my_bid,
            my_ask,
            position,
            balance,
            fee,
            equity: mid.map(|m| balance + position * m - fee),
        });
    }
}

// ---------------------------------------------------------------------------
// Pure delegation, except `elapse`/`elapse_bt`, which sample on the way out.
// ---------------------------------------------------------------------------

impl<MD, B> Bot<MD> for Observed<B>
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
    fn position(&self, a: usize) -> f64 {
        self.inner.position(a)
    }
    fn state_values(&self, a: usize) -> &StateValues {
        self.inner.state_values(a)
    }
    fn depth(&self, a: usize) -> &MD {
        self.inner.depth(a)
    }
    fn last_trades(&self, a: usize) -> &[Event] {
        self.inner.last_trades(a)
    }
    fn clear_last_trades(&mut self, a: Option<usize>) {
        self.inner.clear_last_trades(a)
    }
    fn orders(&self, a: usize) -> &HashMap<OrderId, Order> {
        self.inner.orders(a)
    }

    #[allow(clippy::too_many_arguments)]
    fn submit_buy_order(
        &mut self,
        a: usize,
        id: OrderId,
        price: f64,
        qty: f64,
        tif: TimeInForce,
        ot: OrdType,
        wait: bool,
    ) -> Result<ElapseResult, Self::Error> {
        self.inner
            .submit_buy_order(a, id, price, qty, tif, ot, wait)
    }

    #[allow(clippy::too_many_arguments)]
    fn submit_sell_order(
        &mut self,
        a: usize,
        id: OrderId,
        price: f64,
        qty: f64,
        tif: TimeInForce,
        ot: OrdType,
        wait: bool,
    ) -> Result<ElapseResult, Self::Error> {
        self.inner
            .submit_sell_order(a, id, price, qty, tif, ot, wait)
    }

    fn submit_order(
        &mut self,
        a: usize,
        o: OrderRequest,
        wait: bool,
    ) -> Result<ElapseResult, Self::Error> {
        self.inner.submit_order(a, o, wait)
    }

    fn modify(
        &mut self,
        a: usize,
        id: OrderId,
        price: f64,
        qty: f64,
        wait: bool,
    ) -> Result<ElapseResult, Self::Error> {
        self.inner.modify(a, id, price, qty, wait)
    }

    fn cancel(&mut self, a: usize, id: OrderId, wait: bool) -> Result<ElapseResult, Self::Error> {
        self.inner.cancel(a, id, wait)
    }

    fn clear_inactive_orders(&mut self, a: Option<usize>) {
        self.inner.clear_inactive_orders(a)
    }

    fn wait_order_response(
        &mut self,
        a: usize,
        id: OrderId,
        timeout: i64,
    ) -> Result<ElapseResult, Self::Error> {
        self.inner.wait_order_response(a, id, timeout)
    }

    fn wait_next_feed(
        &mut self,
        include_order_resp: bool,
        timeout: i64,
    ) -> Result<ElapseResult, Self::Error> {
        self.inner.wait_next_feed(include_order_resp, timeout)
    }

    fn elapse(&mut self, duration: i64) -> Result<ElapseResult, Self::Error> {
        let r = self.inner.elapse(duration)?;
        self.sample::<MD>(); // BEFORE the alpha can clear_inactive_orders()
        Ok(r)
    }

    fn elapse_bt(&mut self, duration: i64) -> Result<ElapseResult, Self::Error> {
        let r = self.inner.elapse_bt(duration)?;
        self.sample::<MD>();
        Ok(r)
    }

    fn close(&mut self) -> Result<(), Self::Error> {
        self.sample::<MD>(); // final state, before the engine tears down
        self.inner.close()
    }

    fn feed_latency(&self, a: usize) -> Option<(i64, i64)> {
        self.inner.feed_latency(a)
    }
    fn order_latency(&self, a: usize) -> Option<(i64, i64, i64)> {
        self.inner.order_latency(a)
    }
}
