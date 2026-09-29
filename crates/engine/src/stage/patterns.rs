//! `patterns`: a parsed chart's pattern segments under a layout (architecture §9.2, ADR 0017),
//! from `wolluf-patterns` with the default params. The key is stage-level plus the params hash
//! (section `patterns.k<N>`) and a config hash of the upstream `chart_parse` key, the layout and
//! the rule versions; charts are memoized per md5 in `derivation.input_key`, so no per-chart
//! input enters it. Relabel overrides are not applied here.

use wolluf_chart::Layout;
use wolluf_core::{PatternId, StageId, VersionKey, VersionKeyBuilder};
use wolluf_patterns::{ChartView, PatternParams, rules, segment};

use super::chart_parse;
use crate::error::EngineError;

/// Re-exported so callers above the engine (app may not depend on chart or patterns, D1) can
/// name the stage's input and output.
pub use wolluf_chart::Chart;
pub use wolluf_patterns::Segment;

pub const STAGE: StageId = StageId::from_static("patterns");
pub const VERSION: u32 = 1;

const CONFIG_TAG: &[u8] = b"wolluf.patterns.config.v1";

pub fn run(chart: &Chart, layout: &Layout) -> Result<Vec<Segment>, EngineError> {
    let params = PatternParams::default();
    let view = ChartView::new(chart, layout, &params)?;
    Ok(segment(&view, &params))
}

/// The stage-level key of `segment` rows under `layout`.
pub fn vkey(layout: &Layout) -> Result<VersionKey, EngineError> {
    vkey_with(
        chart_parse::vkey()?,
        layout,
        &PatternParams::default(),
        &rule_versions(),
    )
}

fn rule_versions() -> Vec<(PatternId, u32)> {
    rules::all().iter().map(|r| (r.id(), r.version())).collect()
}

fn vkey_with(
    upstream: VersionKey,
    layout: &Layout,
    params: &PatternParams,
    rules: &[(PatternId, u32)],
) -> Result<VersionKey, EngineError> {
    Ok(VersionKeyBuilder::new(STAGE, VERSION)
        .section(
            format!("patterns.k{}", layout.keymode().columns()),
            params.params_hash(),
        )
        .config(config_hash(upstream, layout, rules))
        .finish()?)
}

/// Fields in the ADR 0006 encoding: the `chart_parse` key the segments were computed from (a
/// new parse re-keys them), the layout (it decides hands and thumb columns), and every rule's
/// `(id, version)` sorted by id (a rule change re-keys them even when the params do not move).
fn config_hash(upstream: VersionKey, layout: &Layout, rules: &[(PatternId, u32)]) -> [u8; 32] {
    let mut sorted: Vec<&(PatternId, u32)> = rules.iter().collect();
    sorted.sort();
    let mut hasher = blake3::Hasher::new();
    hasher.update(CONFIG_TAG);
    field(&mut hasher, &upstream.0);
    field(&mut hasher, layout.id().as_bytes());
    hasher.update(&u32::from(layout.is_mirrored()).to_le_bytes());
    hasher.update(
        &u32::try_from(sorted.len())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    for (id, version) in sorted {
        field(&mut hasher, id.as_str().as_bytes());
        hasher.update(&version.to_le_bytes());
    }
    *hasher.finalize().as_bytes()
}

/// ADR 0006 byte-string field: `u32` little-endian length, then the bytes.
fn field(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_le_bytes());
    hasher.update(bytes);
}

/// One keymode profile's patterns stage: its layout and key, computed once per job.
#[derive(Debug, Clone)]
pub struct Segmenter {
    layout: Layout,
    vkey: VersionKey,
}

impl Segmenter {
    pub fn new(layout: Layout) -> Result<Self, EngineError> {
        let vkey = vkey(&layout)?;
        Ok(Self { layout, vkey })
    }

    pub fn vkey(&self) -> VersionKey {
        self.vkey
    }

    pub fn layout_id(&self) -> &str {
        self.layout.id()
    }

    pub fn run(&self, chart: &Chart) -> Result<Vec<Segment>, EngineError> {
        run(chart, &self.layout)
    }
}

#[cfg(test)]
mod tests {
    use wolluf_chart::testkit::chart_from_rows;
    use wolluf_chart::{Layout, chart};
    use wolluf_core::{Keymode, PatternId};
    use wolluf_patterns::PatternParams;

    use super::*;

    fn layout(id: &str) -> Layout {
        Layout::by_id(id).unwrap()
    }

    /// 4 s of jumpstream: one segment under the default params.
    fn jumpstream() -> wolluf_chart::Chart {
        let cycle = [
            "x......", "..xx...", "x......", ".x..x..", "..x....", "x....x.", "...x...", ".x..x..",
        ];
        let rows: Vec<&str> = cycle.iter().copied().cycle().take(40).collect();
        chart_from_rows(0, 100, &rows).unwrap()
    }

    #[test]
    fn stage_id_and_version_are_stable() {
        assert_eq!(STAGE.as_str(), "patterns");
        assert_eq!(VERSION, 1);
    }

    fn rule_versions() -> Vec<(PatternId, u32)> {
        wolluf_patterns::rules::all()
            .iter()
            .map(|r| (r.id(), r.version()))
            .collect()
    }

    #[test]
    fn vkey_depends_on_layout_and_params_and_is_frozen() {
        let right = layout("k7.313_right_thumb");
        let key = vkey(&right).unwrap();
        assert_eq!(key, vkey(&right).unwrap());
        assert_ne!(key, vkey(&layout("k7.313_left_thumb")).unwrap());
        assert_ne!(key, vkey(&right.mirror()).unwrap());
        let upstream = chart_parse::vkey().unwrap();
        let mut params = PatternParams::default();
        params.segment.min_len_us += 1;
        assert_ne!(
            key,
            vkey_with(upstream, &right, &params, &rule_versions()).unwrap()
        );
        assert_eq!(
            key,
            vkey_with(
                upstream,
                &right,
                &PatternParams::default(),
                &rule_versions()
            )
            .unwrap()
        );
        // Frozen: a change re-keys every stored segment.
        assert_eq!(
            key.to_string(),
            "bc9713f97fca3aa7f07526c4f5c87e928aff9ee8cb02bc5945d2d4383edde680"
        );
    }

    #[test]
    fn upstream_key_and_rule_versions_enter_the_vkey() {
        let right = layout("k7.313_right_thumb");
        let params = PatternParams::default();
        let rules = rule_versions();
        let key = |upstream, rules: &[(PatternId, u32)]| {
            vkey_with(upstream, &right, &params, rules).unwrap()
        };
        let upstream = chart_parse::vkey().unwrap();
        let base = key(upstream, &rules);
        assert_ne!(
            base,
            key(VersionKey([9; 32]), &rules),
            "another chart_parse key"
        );

        let mut bumped = rules.clone();
        bumped[0].1 += 1;
        assert_ne!(base, key(upstream, &bumped), "a rule version bump");
        assert_ne!(base, key(upstream, &rules[1..]), "a rule removed");
        let mut reordered = rules.clone();
        reordered.reverse();
        assert_eq!(base, key(upstream, &reordered), "rule order is canonical");
    }

    #[test]
    fn run_segments_the_chart_under_the_layout() {
        let segments = run(&jumpstream(), &layout("k7.313_right_thumb")).unwrap();
        let primaries: Vec<&str> = segments.iter().map(|s| s.primary.as_str()).collect();
        assert_eq!(primaries, ["regular.stream.jumpstream"]);
        assert_eq!(segments[0].axis.as_str(), "7k.regular.stream");
    }

    #[test]
    fn keymodes_without_an_axis_table_yield_no_segments() {
        let k4 = chart![step = 100; "x...", ".x..", "..x.", "...x"];
        assert_eq!(run(&k4, &Layout::default_for(Keymode::K4)).unwrap(), []);
    }

    #[test]
    fn a_layout_of_another_keymode_is_an_error() {
        let err = run(&jumpstream(), &Layout::default_for(Keymode::K4)).unwrap_err();
        assert!(matches!(err, EngineError::Patterns(_)), "{err}");
    }

    #[test]
    fn segmenter_bundles_layout_key_and_run() {
        let s = Segmenter::new(layout("k7.313_left_thumb")).unwrap();
        assert_eq!(s.layout_id(), "k7.313_left_thumb");
        assert_eq!(s.vkey(), vkey(&layout("k7.313_left_thumb")).unwrap());
        assert_eq!(
            s.run(&jumpstream()).unwrap(),
            run(&jumpstream(), &layout("k7.313_left_thumb")).unwrap()
        );
    }
}
