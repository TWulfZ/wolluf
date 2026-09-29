use std::collections::BTreeSet;

use super::*;

const AXES_7K: [&str; 8] = [
    "7k.regular.jack",
    "7k.regular.tech",
    "7k.regular.speed",
    "7k.regular.stream",
    "7k.ln.general",
    "7k.ln.tech",
    "7k.ln.inverse",
    "7k.ln.release",
];

#[test]
fn taxonomy_ids_and_keys_are_unique() {
    let ids: BTreeSet<&str> = k7().iter().map(|p| p.id.as_str()).collect();
    let keys: BTreeSet<&str> = k7().iter().map(|p| p.key).collect();
    assert_eq!(ids.len(), k7().len());
    assert_eq!(keys.len(), k7().len());
}

#[test]
fn taxonomy_covers_every_axis_and_nothing_else() {
    let covered: BTreeSet<&str> = k7().iter().map(|p| p.axis.as_str()).collect();
    assert_eq!(covered, AXES_7K.into_iter().collect());
    let axes: Vec<&str> = axes(k7()).iter().map(|a| a.as_str()).collect();
    assert_eq!(axes, AXES_7K, "first-seen order");
}

#[test]
fn taxonomy_pattern_ids_extend_their_axis() {
    for p in k7() {
        let axis = p.axis.as_str().strip_prefix("7k.").unwrap();
        let pattern =
            p.id.as_str()
                .strip_prefix(axis)
                .unwrap_or_else(|| panic!("{}", p.id));
        assert!(
            pattern.starts_with('.') && !pattern[1..].contains('.'),
            "{}",
            p.id
        );
    }
}

#[test]
fn taxonomy_keys_are_short_lowercase_and_described() {
    for p in k7() {
        assert!((1..=3).contains(&p.key.len()), "{}", p.key);
        assert!(
            p.key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
            "{}",
            p.key
        );
        assert!(!p.description.is_empty() && !p.description.contains('\n'));
    }
}

// Persisted ids: this list only ever grows (docs/conventions.md, stable ids).
#[test]
fn taxonomy_ids_are_frozen() {
    let ids: Vec<&str> = k7().iter().map(|p| p.id.as_str()).collect();
    insta::assert_snapshot!("taxonomy_k7_ids", ids.join("\n"));
}

#[test]
fn taxonomy_lookup_by_id_and_key() {
    let minijack = by_id(k7(), "regular.jack.minijack").unwrap();
    assert_eq!(minijack.axis.as_str(), "7k.regular.jack");
    assert_eq!(by_key(k7(), minijack.key), Some(minijack));
    assert!(by_id(k7(), "regular.jack").is_none());
    assert!(by_key(k7(), "zzz").is_none());
    assert_eq!(resolve(k7(), "regular.jack.minijack"), Some(minijack));
    assert_eq!(resolve(k7(), minijack.key), Some(minijack));
}

#[test]
fn taxonomy_k7_profile_carries_it() {
    let k7_profile = crate::profile::Registry::builtin()
        .profile(wolluf_core::Keymode::K7)
        .unwrap();
    assert_eq!(k7_profile.taxonomy, k7());
}

// The pilot's vocabulary (2026-09-29): speed is a difficulty dimension, not a shape.
#[test]
fn taxonomy_vocabulary_decisions() {
    for gone in [
        "regular.jack.jackspeed",
        "ln.inverse.inverse",
        "ln.release.release",
    ] {
        assert!(by_id(k7(), gone).is_none(), "{gone}");
    }
    let axis = |id: &str| by_id(k7(), id).unwrap().axis.as_str();
    assert_eq!(axis("regular.stream.chordbracket"), "7k.regular.stream");
    assert_eq!(axis("ln.inverse.gap"), "7k.ln.inverse");
    assert_eq!(axis("ln.release.timing"), "7k.ln.release");
    let desc = |id: &str| by_id(k7(), id).unwrap().description;
    assert!(desc("regular.jack.minijack").contains("exactly two"));
    assert!(desc("regular.jack.longjack").contains("three or more"));
    assert!(desc("regular.stream.bracket").contains("two or more trills"));
    for id in ["regular.stream.jumptrill", "regular.stream.chordbracket"] {
        assert!(desc(id).contains("more than four"), "{id}");
    }
}
