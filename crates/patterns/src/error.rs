//! Pattern engine errors (architecture §7: one `thiserror` enum per crate).

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PatternsError {
    #[error("layout is for {layout}K but the chart is {chart}K")]
    KeymodeMismatch { chart: u8, layout: u8 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymode_mismatch_names_both_sides() {
        let err = PatternsError::KeymodeMismatch {
            chart: 7,
            layout: 4,
        };
        assert_eq!(err.to_string(), "layout is for 4K but the chart is 7K");
    }
}
