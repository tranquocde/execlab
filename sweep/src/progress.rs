//! Live progress for a running sweep.
//!
//! Granularity is one SIMULATION (one session x one param), not one
//! combination — a combination only completes at the batch barrier, so
//! counting combinations would leave the bar frozen for the whole batch.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    sync::Mutex,
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

/// Optional machine-readable progress for UIs. One tick is one completed
/// session x parameter simulation, matching the terminal progress exactly.
pub struct MachineProgress {
    path: Option<PathBuf>,
    completed: AtomicUsize,
    failed: AtomicUsize,
    active: AtomicUsize,
    total: usize,
    started: Instant,
    step: usize,
    write_lock: Mutex<()>,
}

impl MachineProgress {
    pub fn new(path: Option<&Path>, total: usize) -> Self {
        let progress = Self {
            path: path.map(Path::to_path_buf),
            completed: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            total,
            started: Instant::now(),
            step: (total / 200).max(1),
            write_lock: Mutex::new(()),
        };
        progress.write(false);
        progress
    }

    pub fn begin(&self) {
        self.active.fetch_add(1, Ordering::SeqCst);
    }

    pub fn finish(&self, failed: bool) {
        self.active.fetch_sub(1, Ordering::SeqCst);
        if failed {
            self.failed.fetch_add(1, Ordering::SeqCst);
        } else {
            self.completed.fetch_add(1, Ordering::SeqCst);
        }
        let processed = self.completed.load(Ordering::SeqCst) + self.failed.load(Ordering::SeqCst);
        if processed % self.step == 0 || processed == self.total {
            self.write(processed == self.total);
        }
    }

    pub fn skip(&self, count: usize) {
        self.completed.fetch_add(count, Ordering::SeqCst);
        self.write(false);
    }

    pub fn complete(&self) {
        self.write(true);
    }

    fn write(&self, finished: bool) {
        let Some(path) = &self.path else { return };
        let _guard = self.write_lock.lock().unwrap();
        let completed = self.completed.load(Ordering::SeqCst);
        let failed = self.failed.load(Ordering::SeqCst);
        let running = self.active.load(Ordering::SeqCst);
        let processed = completed + failed;
        let pending = self.total.saturating_sub(processed + running);
        let elapsed_sec = self.started.elapsed().as_secs_f64();
        let rate = if elapsed_sec > 0.0 {
            processed as f64 / elapsed_sec
        } else {
            0.0
        };
        let eta_sec = if rate > 0.0 {
            self.total.saturating_sub(processed) as f64 / rate
        } else {
            0.0
        };
        let value = serde_json::json!({
            "total": self.total,
            "completed": completed,
            "failed": failed,
            "running": running,
            "pending": pending,
            "processed": processed,
            "percent": if self.total == 0 { 0.0 } else { processed as f64 / self.total as f64 * 100.0 },
            "elapsed_sec": elapsed_sec,
            "eta_sec": eta_sec,
            "rate_per_sec": rate,
            "finished": finished,
        });
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let temp = path.with_extension("tmp");
        if fs::write(&temp, serde_json::to_vec(&value).unwrap()).is_ok() {
            let _ = fs::rename(temp, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MachineProgress;
    use std::{
        fs, process,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn machine_progress_counts_completed_failed_and_pending() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("execlab-progress-{}-{unique}.json", process::id()));
        let progress = MachineProgress::new(Some(&path), 3);
        progress.begin();
        progress.finish(false);
        progress.begin();
        progress.finish(true);

        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["total"], 3);
        assert_eq!(value["completed"], 1);
        assert_eq!(value["failed"], 1);
        assert_eq!(value["processed"], 2);
        assert_eq!(value["pending"], 1);
        assert_eq!(value["running"], 0);
        let _ = fs::remove_file(path);
    }
}
