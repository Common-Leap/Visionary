//! Transfer live mesh masks across a model layout change. Native group updates remain active.
use std::collections::BTreeMap;

/// Keep a named override only when all of its submeshes agree. Mesh indices and
/// submesh counts can change on import; an inconsistent group cannot be applied
/// through the native name-based setter without changing its meaning.
pub fn render_overrides(entries: &[(u64, u8)]) -> Vec<(u64, u8)> {
    let mut groups = BTreeMap::new();
    for &(name, flags) in entries {
        let state = (flags >> 2) & 3;
        groups
            .entry(name)
            .and_modify(|old| {
                if *old != Some(state) {
                    *old = None;
                }
            })
            .or_insert(Some(state));
    }
    groups
        .into_iter()
        .filter_map(|(name, state)| state.map(|state| (name, state)))
        .collect()
}

pub fn remap_defaults(old: &[(u64, bool)], names: impl IntoIterator<Item = u64>) -> Vec<u8> {
    let defaults: BTreeMap<_, _> = old.iter().copied().collect();
    names
        .into_iter()
        .map(|name| u8::from(defaults.get(&name).copied().unwrap_or(true)))
        .collect()
}

pub fn remap_masks(old: &[(u64, u8)], names: impl IntoIterator<Item = u64>) -> Vec<u8> {
    let masks: BTreeMap<_, _> = old.iter().copied().collect();
    // Native mask 2 delegates to model/animation visibility; 0 and 1 override it.
    names
        .into_iter()
        .map(|name| masks.get(&name).copied().unwrap_or(2))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn render_overrides_are_independent_of_animation_and_submesh_order() {
        // Both meshes are animation-visible, but only the first is forced hidden.
        assert_eq!(
            render_overrides(&[(20, 0x11), (10, 0x19), (10, 0x18)]),
            [(10, 2), (20, 0)]
        );
        // Do not flatten different submesh overrides into one named override.
        assert_eq!(render_overrides(&[(10, 0x19), (10, 0x11)]), []);
        // A native update can clear the override; it is not a permanent hide list.
        assert_eq!(render_overrides(&[(10, 0x11)]), [(10, 0)]);
    }
    #[test]
    fn untracked_hidden_props_and_original_only_faces_keep_their_defaults() {
        let original = [(10, true), (20, false), (30, false)];
        // A new body mesh defaults visible; an unanimated prop stays hidden.
        assert_eq!(remap_defaults(&original, [40, 20, 10]), [1, 0, 1]);
        // Restore from the original defaults, including a face absent in the preview.
        assert_eq!(remap_defaults(&original, [30, 10, 20]), [0, 1, 0]);
    }
    #[test]
    fn reordered_weapons_keep_masks_and_new_meshes_remain_animation_controlled() {
        assert_eq!(
            remap_masks(&[(10, 0), (20, 1), (30, 2)], [30, 20, 40, 10]),
            [2, 1, 2, 0]
        );
    }
    #[test]
    fn later_selection_is_used_when_restoring_a_different_mesh_layout() {
        let preview_names = [30, 20, 10];
        let mut live = remap_masks(&[(10, 0), (20, 1), (30, 2)], preview_names);
        // Native gameplay hides one weapon and selects another after preview begins.
        live[1] = 0;
        live[2] = 1;
        let current: Vec<_> = preview_names.into_iter().zip(live).collect();
        assert_eq!(remap_masks(&current, [10, 20, 30]), [1, 0, 2]);
    }
}
