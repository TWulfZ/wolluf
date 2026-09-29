//! Versioned derivation stages (architecture §5.5). Each stage pins `(VERSION, golden)` in
//! `stage_versions.lock`; the goldens hash quantised outputs over the synthetic fixtures.

pub mod chart_label;
pub mod chart_parse;
#[cfg(any(test, feature = "test-support"))]
mod golden;
pub mod patterns;

use wolluf_core::StageId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageInfo {
    pub id: StageId,
    pub version: u32,
}

/// Every stage, sorted by id.
pub const REGISTERED: &[StageInfo] = &[
    StageInfo {
        id: chart_label::STAGE,
        version: chart_label::VERSION,
    },
    StageInfo {
        id: chart_parse::STAGE,
        version: chart_parse::VERSION,
    },
    StageInfo {
        id: patterns::STAGE,
        version: patterns::VERSION,
    },
];

pub fn registered() -> &'static [StageInfo] {
    REGISTERED
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageGolden {
    pub id: StageId,
    pub version: u32,
    /// 64 lowercase hex chars (blake3).
    pub golden: String,
}

/// What `cargo xtask stage-lock` pins, in `registered()` order.
#[cfg(any(test, feature = "test-support"))]
pub fn goldens() -> Vec<StageGolden> {
    vec![
        StageGolden {
            id: chart_label::STAGE,
            version: chart_label::VERSION,
            golden: golden::chart_label(),
        },
        StageGolden {
            id: chart_parse::STAGE,
            version: chart_parse::VERSION,
            golden: golden::chart_parse(),
        },
        StageGolden {
            id: patterns::STAGE,
            version: patterns::VERSION,
            golden: golden::patterns(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_stages_are_sorted_unique_and_stable() {
        let ids: Vec<(&str, u32)> = registered()
            .iter()
            .map(|s| (s.id.as_str(), s.version))
            .collect();
        assert_eq!(
            ids,
            [("chart_label", 1), ("chart_parse", 1), ("patterns", 1)]
        );
    }

    #[test]
    fn goldens_cover_every_stage_and_are_deterministic() {
        let first = goldens();
        assert_eq!(first, goldens());
        // A stage missing from either list would escape the lock.
        let pinned: Vec<StageInfo> = first
            .iter()
            .map(|g| StageInfo {
                id: g.id.clone(),
                version: g.version,
            })
            .collect();
        assert_eq!(pinned, registered());
        for g in &first {
            assert_eq!(g.golden.len(), 64, "{}", g.id);
            assert!(
                g.golden
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "{}",
                g.id
            );
        }
        assert_ne!(first[0].golden, first[1].golden);
        assert_ne!(first[1].golden, first[2].golden);
    }
}
