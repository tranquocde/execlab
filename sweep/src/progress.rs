//! Live progress for a running sweep.
//!
//! Granularity is one SIMULATION (one session x one param), not one
//! combination — a combination only completes at the batch barrier, so
//! counting combinations would leave the bar frozen for the whole batch.

use std::{
    io::{self, Write},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

const BAR_WIDTH: usize = 28;

pub struct Progress {
    done: AtomicUsize,
    total: usize,
    started: Instant,
    label: String,
    /// Redraw every `step` ticks — writing to stderr from every worker on every
    /// simulation would cost more than the simulations.
    step: usize,
}

impl Progress {
    pub fn new(label: impl Into<String>, total: usize) -> Self {
        let p = Self {
            done: AtomicUsize::new(0),
            total,
            started: Instant::now(),
            label: label.into(),
            step: (total / 200).max(1),
        };
        p.draw(0);
        p
    }

    /// Called from worker threads; cheap in the common case.
    pub fn inc(&self) {
        let n = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        if n % self.step == 0 || n == self.total {
            self.draw(n);
        }
    }

    fn draw(&self, n: usize) {
        let frac = if self.total == 0 {
            1.0
        } else {
            n as f64 / self.total as f64
        };
        let filled = (frac * BAR_WIDTH as f64) as usize;
        let elapsed = self.started.elapsed().as_secs_f64();
        let rate = if elapsed > 0.0 {
            n as f64 / elapsed
        } else {
            0.0
        };
        let eta = if rate > 0.0 {
            (self.total - n) as f64 / rate
        } else {
            0.0
        };

        eprint!(
            "\r{label} [{bar}{pad}] {n}/{total} sims | {rate:.0}/s | eta {eta:.0}s   ",
            label = self.label,
            bar = "#".repeat(filled),
            pad = " ".repeat(BAR_WIDTH - filled),
            total = self.total,
        );
        let _ = io::stderr().flush();
    }

    pub fn finish(&self) {
        self.draw(self.done.load(Ordering::Relaxed));
        eprintln!();
    }
}
