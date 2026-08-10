//! Deterministic physical-loadout and target-plate planning.
//!
//! The planner deliberately owns a small, serde-ready input model instead of
//! depending on the parser's intermediate representation. This keeps parsing,
//! color matching, and output writing replaceable at their crate boundaries.

mod model;
mod planning;

pub use model::*;
pub use planning::plan;
