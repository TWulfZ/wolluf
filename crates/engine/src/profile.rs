//! Keymode profiles (architecture §3, D5): which keymodes the engine analyses and with what
//! defaults. Profiles are data; a keymode without one is not indexed.

use wolluf_chart::Layout;
use wolluf_chart::layout::DEFAULT_K7;
use wolluf_core::Keymode;

use crate::taxonomy::{self, PatternDef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeymodeProfile {
    pub keymode: Keymode,
    /// A `wolluf_chart::layout` preset id.
    pub default_layout: &'static str,
    /// Whether difficulty-name labels (`crate::labels`) are extracted for this keymode.
    pub label_sources: bool,
    /// The keymode's pattern ids (`crate::taxonomy`).
    pub taxonomy: &'static [PatternDef],
}

impl KeymodeProfile {
    /// Falls back to the keymode's first preset if the id stops resolving, so a profile always
    /// yields a layout of its own keymode.
    pub fn layout(&self) -> Layout {
        Layout::by_id(self.default_layout)
            .filter(|l| l.keymode() == self.keymode)
            .unwrap_or_else(|| Layout::default_for(self.keymode))
    }

    /// A user-chosen preset, only if it belongs to this profile's keymode.
    pub fn layout_by_id(&self, id: &str) -> Option<Layout> {
        Layout::by_id(id).filter(|l| l.keymode() == self.keymode)
    }
}

const BUILTIN: &[KeymodeProfile] = &[KeymodeProfile {
    keymode: Keymode::K7,
    default_layout: DEFAULT_K7,
    label_sources: true,
    taxonomy: taxonomy::k7(),
}];

/// The concrete set of keymodes the engine knows (D5); features never hard-code it.
#[derive(Debug, Clone, Copy)]
pub struct Registry {
    profiles: &'static [KeymodeProfile],
}

impl Registry {
    pub const fn builtin() -> Self {
        Self { profiles: BUILTIN }
    }

    /// Enabled profiles, ascending by keymode.
    pub fn profiles(&self) -> &'static [KeymodeProfile] {
        self.profiles
    }

    pub fn profile(&self, keymode: Keymode) -> Option<&'static KeymodeProfile> {
        self.profiles.iter().find(|p| p.keymode == keymode)
    }
}

#[cfg(test)]
mod tests {
    use wolluf_chart::layout::DEFAULT_K7;
    use wolluf_core::Keymode;

    use super::*;

    #[test]
    fn builtin_enables_k7_with_the_pilot_layout_and_label_sources() {
        let registry = Registry::builtin();
        let k7 = registry.profile(Keymode::K7).unwrap();
        assert_eq!(k7.keymode, Keymode::K7);
        assert_eq!(k7.default_layout, "k7.313_right_thumb");
        assert_eq!(k7.default_layout, DEFAULT_K7);
        assert!(k7.label_sources);
        let layout = k7.layout();
        assert_eq!(layout.id(), DEFAULT_K7);
        assert_eq!(layout.keymode(), Keymode::K7);
    }

    #[test]
    fn layout_by_id_resolves_only_presets_of_the_profile_keymode() {
        let k7 = Registry::builtin().profile(Keymode::K7).unwrap();
        assert_eq!(
            k7.layout_by_id("k7.313_left_thumb")
                .map(|l| l.id().to_owned()),
            Some("k7.313_left_thumb".to_owned())
        );
        assert!(k7.layout_by_id("k4.generic").is_none());
        assert!(k7.layout_by_id("nope").is_none());
    }

    #[test]
    fn keymodes_without_a_profile_are_disabled() {
        let registry = Registry::builtin();
        assert!(registry.profile(Keymode::K4).is_none());
        let keymodes: Vec<Keymode> = registry.profiles().iter().map(|p| p.keymode).collect();
        assert_eq!(keymodes, [Keymode::K7]);
    }
}
