//! Which of a fighter's alternative meshes show, for the 3D preview.
//!
//! A fighter model carries every variant of a part at once: Link has his sword in his hand
//! and the same sword in its sheath, his shield on his arm and on his back. The game shows
//! one of each set. `param/expression_vl.prc` lists the sets (`param_visibility_data_body`):
//! each set's groups name the mesh nodes they show, and `reset` names the group shown until a
//! script picks another with `VisibilityModule::set_int64(set, group)`. Without this the
//! preview drew both swords and both shields.

use std::collections::HashSet;
use std::path::Path;

use prc::hash40::{hash40 as to_hash40, Hash40};
use prc::ParamKind;

/// One set of alternatives: its groups (each with the mesh nodes it shows) and the group shown
/// by default.
#[derive(Debug, Clone, PartialEq)]
pub struct VisibilitySet {
    pub set: u64,
    pub groups: Vec<(u64, Vec<u64>)>,
    pub default: u64,
}

/// The sets in a fighter's `expression_vl.prc`; empty when it has none.
pub fn load(path: &Path) -> Vec<VisibilitySet> {
    let Ok(root) = prc::open(path) else {
        return Vec::new();
    };
    let find = |s: &prc::ParamStruct, key: &str| {
        let hash = to_hash40(key);
        s.0.iter().find(|(k, _)| *k == hash).map(|(_, v)| v.clone())
    };
    let hash_of = |p: Option<ParamKind>| match p {
        Some(ParamKind::Hash(Hash40(h))) => Some(h),
        _ => None,
    };
    let Some(ParamKind::Struct(body)) = find(&root, "param_visibility_data_body") else {
        return Vec::new();
    };
    let Some(ParamKind::List(sets)) = find(&body, "param_set") else {
        return Vec::new();
    };
    sets.0
        .iter()
        .filter_map(|entry| {
            let ParamKind::Struct(entry) = entry else {
                return None;
            };
            let set = hash_of(find(entry, "no"))?;
            let default = hash_of(find(entry, "reset"))?;
            let Some(ParamKind::List(groups)) = find(entry, "param_group") else {
                return None;
            };
            let groups = groups
                .0
                .iter()
                .filter_map(|group| {
                    let ParamKind::Struct(group) = group else {
                        return None;
                    };
                    let name = hash_of(find(group, "group"))?;
                    let nodes = match find(group, "param_node") {
                        Some(ParamKind::List(nodes)) => nodes
                            .0
                            .into_iter()
                            .filter_map(|n| hash_of(Some(n)))
                            .collect(),
                        _ => Vec::new(),
                    };
                    Some((name, nodes))
                })
                .collect();
            Some(VisibilitySet {
                set,
                groups,
                default,
            })
        })
        .collect()
}

/// The mesh nodes (hash40 of the node name) hidden at a frame, given the move's visibility
/// changes up to it as (frame, set, group) in running order.
pub fn hidden_nodes(
    sets: &[VisibilitySet],
    changes: &[(u32, String, String)],
    frame: u32,
) -> HashSet<u64> {
    let mut hidden = HashSet::new();
    for set in sets {
        let mut active = set.default;
        for (at, name, group) in changes {
            if *at <= frame && to_hash40(name).0 == set.set {
                active = to_hash40(group).0;
            }
        }
        let shown: HashSet<u64> = set
            .groups
            .iter()
            .filter(|(group, _)| *group == active)
            .flat_map(|(_, nodes)| nodes.iter().copied())
            .collect();
        for (_, nodes) in &set.groups {
            hidden.extend(nodes.iter().copied().filter(|node| !shown.contains(node)));
        }
    }
    hidden
}

/// The node a mesh belongs to: its name before `_VIS` (`link_kenb_VIS_O_OBJShape` is
/// `link_kenb`), as hash40.
pub fn mesh_node(mesh_name: &str) -> u64 {
    let base = mesh_name.split("_VIS").next().unwrap_or(mesh_name);
    to_hash40(base).0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link_sets() -> Vec<VisibilitySet> {
        // Link's param_visibility_data_body, as his expression_vl.prc has it.
        let h = |s: &str| to_hash40(s).0;
        vec![
            VisibilitySet {
                set: h("sword"),
                groups: vec![
                    (h("sword_normal"), vec![h("link_ken"), h("link_sayatuka")]),
                    (h("sword_back"), vec![h("link_kenb"), h("link_sayatuka")]),
                ],
                default: h("sword_normal"),
            },
            VisibilitySet {
                set: h("shield"),
                groups: vec![
                    (h("shield_normal"), vec![h("link_tate")]),
                    (h("shield_back"), vec![h("link_tateb")]),
                ],
                default: h("shield_normal"),
            },
        ]
    }

    #[test]
    fn only_the_active_group_of_each_set_shows() {
        let sets = link_sets();
        // Link's side special: sword to its back on frame 3, back in hand on frame 44.
        let changes = vec![
            (3, "sword".to_owned(), "sword_back".to_owned()),
            (44, "sword".to_owned(), "sword_normal".to_owned()),
        ];
        let hidden = |frame| hidden_nodes(&sets, &changes, frame);
        let h = |s: &str| to_hash40(s).0;
        // Before: sword in hand, shield on arm; the sheathed sword and back shield hidden.
        assert_eq!(hidden(1), HashSet::from([h("link_kenb"), h("link_tateb")]));
        // Sword on his back; the scabbard is in both groups, so it stays.
        assert_eq!(hidden(10), HashSet::from([h("link_ken"), h("link_tateb")]));
        assert_eq!(hidden(50), HashSet::from([h("link_kenb"), h("link_tateb")]));
    }

    #[test]
    fn mesh_names_resolve_to_their_node() {
        assert_eq!(
            mesh_node("link_kenb_VIS_O_OBJShape"),
            to_hash40("link_kenb").0
        );
        assert_eq!(
            mesh_node("link_ken_VIS_O_OBJShape"),
            to_hash40("link_ken").0
        );
    }

    #[test]
    fn link_s_own_file_has_these_sets() {
        let Some(root) = std::env::var_os("VISIONARY_DATA_ROOT") else {
            return;
        };
        let path = std::path::Path::new(&root).join("fighter/link/param/expression_vl.prc");
        assert_eq!(load(&path), link_sets());
    }
}
