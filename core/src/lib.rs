//! The data contract shared by the sweep runner and the visualization.
//!
//! Deliberately thin: types and the output-layout rules, nothing else. If
//! something here ever needs `hftbacktest`, it belongs in `sweep` instead.

pub mod output;
pub mod row;

pub use output::{is_complete, param_hash, DataRef, Manifest};
pub use row::{Results, SessionRow};
