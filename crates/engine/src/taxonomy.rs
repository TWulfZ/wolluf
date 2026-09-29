//! Pattern taxonomy per keymode (architecture §5.1): the persisted pattern ids, their parent
//! axis, a short key for typing labels and a one-line description. Data only; the pattern
//! engine and the gold-set labeller share these ids, which are never renumbered.

use wolluf_core::{AxisId, PatternId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternDef {
    pub id: PatternId,
    pub axis: AxisId,
    /// 1–3 lowercase chars, unique within the keymode.
    pub key: &'static str,
    pub description: &'static str,
}

const fn def(
    id: &'static str,
    axis: &'static str,
    key: &'static str,
    description: &'static str,
) -> PatternDef {
    PatternDef {
        id: PatternId::from_static(id),
        axis: AxisId::from_static(axis),
        key,
        description,
    }
}

const JACK: &str = "7k.regular.jack";
const TECH: &str = "7k.regular.tech";
const SPEED: &str = "7k.regular.speed";
const STREAM: &str = "7k.regular.stream";
const LN_GENERAL: &str = "7k.ln.general";
const LN_TECH: &str = "7k.ln.tech";
const LN_INVERSE: &str = "7k.ln.inverse";
const LN_RELEASE: &str = "7k.ln.release";

/// The 7K Regular axes are jack, tech, speed and stream; the LN axes general, tech, inverse and
/// release (stamina is derived, not an axis).
static K7: [PatternDef; 25] = [
    def(
        "regular.jack.minijack",
        JACK,
        "mj",
        "exactly two consecutive notes in one column",
    ),
    def(
        "regular.jack.chordjack",
        JACK,
        "cj",
        "chords that repeat columns row after row",
    ),
    def(
        "regular.jack.longjack",
        JACK,
        "lj",
        "three or more consecutive notes in one column",
    ),
    def(
        "regular.jack.anchor",
        JACK,
        "a",
        "one column recurring every other row under a stream",
    ),
    def(
        "regular.tech.irregular",
        TECH,
        "ir",
        "off-snap, polyrhythmic or mixed-timing notes",
    ),
    def(
        "regular.tech.hand_imbalance",
        TECH,
        "hi",
        "density loaded on one hand",
    ),
    def(
        "regular.tech.thumb",
        TECH,
        "th",
        "patterns that load column 4 heavily",
    ),
    def(
        "regular.speed.burst",
        SPEED,
        "bu",
        "short bursts much faster than the surrounding section",
    ),
    def(
        "regular.stream.single",
        STREAM,
        "st",
        "single-note stream, no chords and no rolls",
    ),
    def(
        "regular.stream.jumpstream",
        STREAM,
        "js",
        "stream with two-note chords mixed in",
    ),
    def(
        "regular.stream.handstream",
        STREAM,
        "hs",
        "stream with three-note chords mixed in",
    ),
    def(
        "regular.stream.chordstream_light",
        STREAM,
        "cl",
        "stream of small chords, mostly two to three notes",
    ),
    def(
        "regular.stream.chordstream_dense",
        STREAM,
        "cd",
        "stream of large chords, four notes or more",
    ),
    def(
        "regular.stream.roll",
        STREAM,
        "r",
        "rolls and stairs sweeping across the columns",
    ),
    def(
        "regular.stream.trill",
        STREAM,
        "t",
        "two columns alternating",
    ),
    def(
        "regular.stream.jumptrill",
        STREAM,
        "jt",
        "two chords alternating, including alternating chords of more than four notes",
    ),
    def(
        "regular.stream.split_trill",
        STREAM,
        "spt",
        "trill split across both hands",
    ),
    def(
        "regular.stream.bracket",
        STREAM,
        "b",
        "two or more trills at the same time within one hand",
    ),
    // Interlude's "Brackets"; wolluf keeps `bracket` for the wiki/MinaCalc meaning.
    def(
        "regular.stream.chordbracket",
        STREAM,
        "cb",
        "a two- or three-note chord shape moving across columns without jacking; alternating \
         chords of more than four notes are a jumptrill",
    ),
    def(
        "ln.general.density",
        LN_GENERAL,
        "ld",
        "dense long notes with short gaps between them",
    ),
    def(
        "ln.general.chord",
        LN_GENERAL,
        "lc",
        "long-note chords pressed or released together",
    ),
    def(
        "ln.tech.hybrid",
        LN_TECH,
        "lh",
        "long notes held while tapping rice notes",
    ),
    def(
        "ln.tech.shield",
        LN_TECH,
        "ls",
        "a tap right before a long-note head on the same column",
    ),
    def(
        "ln.inverse.gap",
        LN_INVERSE,
        "li",
        "inverse: long notes fill the gaps between notes",
    ),
    def(
        "ln.release.timing",
        LN_RELEASE,
        "lr",
        "release timing: tails that need precise, staggered releases",
    ),
];

pub const fn k7() -> &'static [PatternDef] {
    &K7
}

pub fn by_id<'a>(taxonomy: &'a [PatternDef], id: &str) -> Option<&'a PatternDef> {
    taxonomy.iter().find(|p| p.id.as_str() == id)
}

pub fn by_key<'a>(taxonomy: &'a [PatternDef], key: &str) -> Option<&'a PatternDef> {
    taxonomy.iter().find(|p| p.key == key)
}

/// A full id or a short key.
pub fn resolve<'a>(taxonomy: &'a [PatternDef], token: &str) -> Option<&'a PatternDef> {
    by_id(taxonomy, token).or_else(|| by_key(taxonomy, token))
}

/// Distinct axes in first-seen order.
pub fn axes(taxonomy: &[PatternDef]) -> Vec<&AxisId> {
    let mut out: Vec<&AxisId> = Vec::new();
    for p in taxonomy {
        if !out.contains(&&p.axis) {
            out.push(&p.axis);
        }
    }
    out
}

#[cfg(test)]
mod tests;
