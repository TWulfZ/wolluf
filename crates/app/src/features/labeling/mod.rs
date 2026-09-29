//! Gold-set labelling (F1): blind, stratified sampling of chart windows and the user's
//! pattern labels as append-only feedback events (architecture §5.3, §6.1).

pub mod dto;
pub mod sampler;
mod service;
pub mod window;

pub use service::LabelingService;

/// The slice's `label.error.*` message keys; shells map them to their own text.
pub mod keys {
    pub const UNKNOWN_PATTERN: &str = "label.error.unknown_pattern";
    pub const WINDOW_TOO_SHORT: &str = "label.error.window_too_short";
}
