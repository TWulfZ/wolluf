//! Rule-based pattern detection (architecture §3, §9.2; ADR 0017 ids): per-row primitives in
//! [`ChartView`], one [`PatternRule`] per pattern emitting [`Candidate`] spans, and every
//! threshold in [`PatternParams`] (D17); [`segment`] resolves them into one primary pattern per
//! instant. Pure and deterministic (D2, D3).

pub mod axes;
pub mod error;
pub mod params;
pub mod rule;
pub mod rules;
pub mod segment;
pub mod view;

pub use error::PatternsError;
pub use params::PatternParams;
pub use rule::{Candidate, PatternRule};
pub use segment::{Segment, segment};
pub use view::{ChartView, Direction, HandMasks, RowFeat};
