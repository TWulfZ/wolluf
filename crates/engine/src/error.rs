//! Engine errors (architecture §7: one `thiserror` enum per crate).

use thiserror::Error;
use wolluf_chart::ChartError;
use wolluf_core::CoreError;
use wolluf_patterns::PatternsError;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Chart(#[from] ChartError),
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Patterns(#[from] PatternsError),
    #[error("rows blob is empty")]
    EmptyRowsBlob,
    #[error("rows blob format {0} is not supported")]
    UnsupportedRowsFormat(u8),
    #[error("rows blob is corrupt: {0}")]
    CorruptRowsBlob(String),
}
