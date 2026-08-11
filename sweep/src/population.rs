//! Where batches of params come from.
//!
//! Today: `Grid`, which walks the search space in dumb fixed-size chunks and
//! ignores results entirely. Later: `Ga`, which breeds each batch from the last
//! batch's fitness. The runner cannot tell the difference — that is the point.
//! All the plumbing (barrier, row pivot, batch write, resume) gets debugged
//! during the dumb phase, when results are hand-checkable.

use execlab_core::Results;

pub trait Population<P> {
    /// `None` = finished. `last` carries every result so far — ignored by
    /// `Grid`, and the entire input to a GA.
    fn next_batch(&mut self, last: &Results) -> Option<Vec<P>>;

    /// Total combinations, if knowable up front. `Grid` knows; a GA that stops
    /// on a convergence criterion does not, so this returns `None` there and
    /// progress falls back to counting batches.
    fn total_hint(&self) -> Option<usize> {
        None
    }
}

/// Dumb: fixed-size chunks of a precomputed grid.
pub struct Grid<P> {
    space: Vec<P>,
    cursor: usize,
    size: usize,
}

impl<P> Grid<P> {
    pub fn new(space: Vec<P>, size: usize) -> Self {
        let size = size.max(1);
        Self {
            space,
            cursor: 0,
            size,
        }
    }
}

impl<P: Clone> Population<P> for Grid<P> {
    fn next_batch(&mut self, _last: &Results) -> Option<Vec<P>> {
        if self.cursor >= self.space.len() {
            return None;
        }
        let end = (self.cursor + self.size).min(self.space.len());
        let batch = self.space[self.cursor..end].to_vec();
        self.cursor = end;
        Some(batch)
    }

    fn total_hint(&self) -> Option<usize> {
        Some(self.space.len())
    }
}

// ---------------------------------------------------------------------------
// Later — the runner does NOT change, only this impl appears:
//
// pub struct Ga<P> { pop: Vec<P>, generation: usize, max_gen: usize }
//
// impl<P: Clone> Population<P> for Ga<P> {
//     fn next_batch(&mut self, last: &Results) -> Option<Vec<P>> {
//         if self.generation >= self.max_gen { return None; }
//         if self.generation > 0 {
//             self.pop = breed(&self.pop, fitness(last));   // the only new thinking
//         }
//         self.generation += 1;
//         Some(self.pop.clone())
//     }
// }
//
// CAREFUL on resume: an individual whose output directory already exists still
// needs its FITNESS for breeding, even though it needs no re-run. The GA path
// must LOAD the cached parquet rather than skip the combination outright —
// otherwise it breeds from a partial fitness picture and silently searches
// worse, with no error to notice.
// ---------------------------------------------------------------------------
