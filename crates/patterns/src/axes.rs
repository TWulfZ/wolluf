//! Pattern → axis for 7K (ADR 0017). The engine taxonomy holds the same mapping; patterns cannot
//! depend on engine (D1), so it is repeated here and the engine cross-checks it.

use wolluf_core::{AxisId, Keymode, PatternId};

const fn pair(pattern: &'static str, axis: &'static str) -> (PatternId, AxisId) {
    (PatternId::from_static(pattern), AxisId::from_static(axis))
}

const JACK: &str = "7k.regular.jack";
const TECH: &str = "7k.regular.tech";
const SPEED: &str = "7k.regular.speed";
const STREAM: &str = "7k.regular.stream";

pub static K7: [(PatternId, AxisId); 25] = [
    pair("regular.jack.minijack", JACK),
    pair("regular.jack.chordjack", JACK),
    pair("regular.jack.longjack", JACK),
    pair("regular.jack.anchor", JACK),
    pair("regular.tech.irregular", TECH),
    pair("regular.tech.hand_imbalance", TECH),
    pair("regular.tech.thumb", TECH),
    pair("regular.speed.burst", SPEED),
    pair("regular.stream.single", STREAM),
    pair("regular.stream.jumpstream", STREAM),
    pair("regular.stream.handstream", STREAM),
    pair("regular.stream.chordstream_light", STREAM),
    pair("regular.stream.chordstream_dense", STREAM),
    pair("regular.stream.roll", STREAM),
    pair("regular.stream.trill", STREAM),
    pair("regular.stream.jumptrill", STREAM),
    pair("regular.stream.split_trill", STREAM),
    pair("regular.stream.bracket", STREAM),
    pair("regular.stream.chordbracket", STREAM),
    pair("ln.general.density", "7k.ln.general"),
    pair("ln.general.chord", "7k.ln.general"),
    pair("ln.tech.hybrid", "7k.ln.tech"),
    pair("ln.tech.shield", "7k.ln.tech"),
    pair("ln.inverse.gap", "7k.ln.inverse"),
    pair("ln.release.timing", "7k.ln.release"),
];

/// Only 7K has axes so far; other keymodes get their table with their profile (§9.1).
pub fn axis_of(keymode: Keymode, pattern: &PatternId) -> Option<AxisId> {
    if keymode != Keymode::K7 {
        return None;
    }
    K7.iter()
        .find(|(p, _)| p == pattern)
        .map(|(_, axis)| axis.clone())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use wolluf_core::{Keymode, PatternId};

    use super::*;
    use crate::rules;

    #[test]
    fn every_rule_has_exactly_one_k7_axis() {
        let rule_ids: BTreeSet<String> = rules::all().iter().map(|r| r.id().to_string()).collect();
        let table_ids: BTreeSet<String> = K7.iter().map(|(p, _)| p.to_string()).collect();
        assert_eq!(table_ids.len(), K7.len());
        assert_eq!(rule_ids, table_ids);
    }

    #[test]
    fn axes_are_the_adr_0017_axes_named_by_the_pattern_prefix() {
        let axes: BTreeSet<&str> = K7.iter().map(|(_, a)| a.as_str()).collect();
        assert_eq!(
            axes,
            BTreeSet::from([
                "7k.regular.jack",
                "7k.regular.tech",
                "7k.regular.speed",
                "7k.regular.stream",
                "7k.ln.general",
                "7k.ln.tech",
                "7k.ln.inverse",
                "7k.ln.release",
            ])
        );
        for (pattern, axis) in &K7 {
            let family: Vec<&str> = pattern.as_str().split('.').take(2).collect();
            assert_eq!(
                axis.as_str(),
                format!("7k.{}", family.join(".")),
                "{pattern}"
            );
        }
    }

    #[test]
    fn lookup_is_per_keymode() {
        let minijack = PatternId::from_static("regular.jack.minijack");
        assert_eq!(
            axis_of(Keymode::K7, &minijack).map(|a| a.to_string()),
            Some("7k.regular.jack".to_owned())
        );
        assert_eq!(axis_of(Keymode::K4, &minijack), None);
        assert_eq!(
            axis_of(Keymode::K7, &PatternId::from_static("no.such.pattern")),
            None
        );
    }
}
