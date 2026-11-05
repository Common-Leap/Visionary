//! Exact owner/group/status evidence for direct root execute calls in the pinned ACMD cache.
//!
//! Counts are cache occurrences, not native-semantic assertions. Nested or otherwise
//! non-root calls and malformed calls are intentionally absent.

pub(crate) const PINNED_VISIBILITY_PAIR_COUNTS: &[(&str, &str, &str, usize)] = &[
    ("bayonetta", "butterflywing", "butterflywing_hide", 3),
    ("bayonetta", "butterflywing", "butterflywing_show", 3),
    ("bayonetta", "gun_hand", "gun_hand_hide_all", 37),
    ("bayonetta", "gun_hand", "gun_hand_r_hide", 3),
    ("bayonetta", "gun_hand", "gun_hand_show_all", 12),
    ("chrom", "sword", "sword_lhand", 2),
    ("chrom", "sword", "sword_lhand_saya", 2),
    ("chrom", "sword", "sword_rhand_saya", 4),
    ("chrom", "sword", "sword_saya_only", 2),
    ("chrom", "sword", "sword_saya_tsuka", 16),
    ("chrom", "sword", "sword_sword_saya", 12),
    ("cloud", "sword", "none", 5),
    ("cloud", "sword", "sword_l", 4),
    ("cloud", "sword", "sword_r", 3),
    ("cloud", "sword", "visibility_status_none", 4),
    ("daisy", "crown_knuckle", "crown_knuckle_hand", 1),
    ("daisy", "crown_knuckle", "crown_knuckle_head", 1),
    ("daisy", "smash_item", "smash_item_none", 3),
    ("dedede", "body", "body_normal", 77),
    ("dedede", "body", "body_swell", 30),
    ("dedede", "hammer", "hammer_disp_off", 18),
    ("dedede", "hammer", "hammer_normal", 4),
    ("diddy", "barrel", "barrel_invisible", 2),
    ("diddy", "barrel", "barrel_visible", 10),
    ("diddy", "head_shadow", "head_shadow_invisible", 2),
    ("diddy", "head_shadow", "head_shadow_normal", 6),
    ("duckhunt", "body", "body_normal", 350),
    ("duckhunt", "body", "body_openwing", 27),
    ("falco", "blaster", "blaster_hide", 5),
    ("falco", "blaster", "blaster_normal", 5),
    ("fox", "blaster", "blaster_hide", 11),
    ("fox", "blaster", "blaster_normal", 8),
    ("gamewatch", "hand", "hand_hold_l", 64),
    ("gamewatch", "hand", "hand_hold_lr", 144),
    ("gamewatch", "hand", "hand_hold_r", 3),
    ("gamewatch", "hand", "hand_normal", 6),
    ("gamewatch", "head", "head_close", 105),
    ("gamewatch", "head", "head_diver", 5),
    ("gamewatch", "head", "head_none", 44),
    ("gamewatch", "head", "head_sparky", 2),
    ("gamewatch", "lhand", "lhand_alarm", 8),
    ("gamewatch", "lhand", "lhand_key", 2),
    ("gamewatch", "lhand", "lhand_none", 1),
    ("gamewatch", "rhand", "rhand_hammer", 3),
    ("ike", "sword", "sword_hide", 13),
    ("ike", "sword", "sword_normal", 2),
    ("inkling", "tank", "tank_subempty", 2),
    ("jack", "mask", "on", 1),
    ("kamui", "dragon", "dragon_horn", 37),
    ("kamui", "dragon", "dragon_horn_win_3", 2),
    ("kamui", "dragon", "dragon_none", 40),
    ("kamui", "dragon", "dragon_spear_arm_lr", 1),
    ("kamui", "dragon", "dragon_spear_leg_l", 2),
    ("kamui", "dragon", "dragon_spear_leg_lr", 1),
    ("kamui", "dragon", "dragon_wing", 4),
    ("kamui", "dragon", "dragon_wing_no_horn", 7),
    ("kamui", "front_hair", "front_hair_hide", 46),
    ("kamui", "front_hair", "front_hair_normal", 43),
    ("kamui", "hair", "hair_hide", 46),
    ("kamui", "hair", "hair_normal", 43),
    ("kamui", "mantle", "mantle_hide", 1),
    ("kamui", "weapon", "weapon_hide", 4),
    ("kirby", "body", "body_normal", 5),
    ("koopag", "body", "body_normal", 13),
    ("koopag", "body", "body_shell", 14),
    ("koopajr", "clown", "clown_off", 3),
    ("koopajr", "clown_face", "clown_face_mouth_open", 3),
    ("koopajr", "clown_face", "clown_face_off", 3),
    ("koopajr", "item", "item_drill", 3),
    ("koopajr", "item", "item_fork", 5),
    ("koopajr", "item", "item_glove", 8),
    ("koopajr", "item", "item_ironball", 5),
    ("koopajr", "item", "item_none", 1),
    ("koopajr", "item", "item_saw", 2),
    ("koopajr", "item", "item_tongue", 1),
    ("koopajr", "sholder_l", "sholder_l_on", 20),
    ("koopajr", "sholder_r", "sholder_r_on", 20),
    ("koopajr", "tarap", "tarap_off", 6),
    ("krool", "crown", "crown_hide", 18),
    ("krool", "crown", "crown_normal", 10),
    ("link", "shield", "shield_back", 134),
    ("link", "shield", "shield_normal", 58),
    ("link", "sword", "sword_back", 54),
    ("link", "sword", "sword_normal", 27),
    ("littlemac", "swet", "swet_off", 2),
    ("littlemac", "swet", "swet_on", 2),
    ("lucas", "bat", "bat_visible", 4),
    ("lucina", "sword", "sword_normal", 4),
    ("lucina", "sword", "sword_saya", 26),
    ("mariod", "stethoscope", "stethoscope_off", 2),
    ("marth", "sword", "sword_normal", 6),
    ("marth", "sword", "sword_saya", 30),
    ("metaknight", "body", "body_no_sword", 1),
    ("metaknight", "body", "body_normal", 2),
    ("miiswordsman", "sword", "sword_hide", 1),
    ("murabito", "item", "item_axe", 2),
    ("murabito", "item", "item_none", 17),
    ("murabito", "item", "item_scoop", 3),
    ("murabito", "item", "item_shovel", 3),
    ("murabito", "item", "item_sprinkling", 2),
    ("murabito", "item", "item_woodenstick", 5),
    ("nana", "hammer", "hammer_invisible", 13),
    ("ness", "bat", "bat_invisible", 3),
    ("ness", "bat", "bat_visible", 6),
    ("ness", "head", "head_bomb", 2),
    ("ness", "head", "head_normal", 2),
    ("pacman", "body", "body_none", 26),
    ("pacman", "body", "body_normal", 20),
    ("pacman", "fairy", "fairy_disp", 2),
    ("pacman", "fairy", "fairy_disp_off", 2),
    ("pacman", "final_poweresa", "final_poweresa_disp", 2),
    ("pacman", "final_poweresa", "final_poweresa_disp_off", 2),
    ("pacman", "monster", "monster_akabee", 1),
    ("pacman", "monster", "monster_aosuke", 1),
    ("pacman", "monster", "monster_disp_off", 4),
    ("pacman", "monster", "monster_guzuda", 1),
    ("pacman", "monster", "monster_ijike", 1),
    ("pacman", "namco", "namco_disp_off", 2),
    ("pacman", "pizza", "pizza_none", 19),
    ("pacman", "pizza", "pizza_normal", 25),
    ("peach", "crown_knuckle", "crown_knuckle_hand", 1),
    ("peach", "crown_knuckle", "crown_knuckle_head", 1),
    ("peach", "smash_item", "smash_item_none", 3),
    ("pfushigisou", "weapon", "invalid", 3),
    ("pfushigisou", "weapon", "weapon_normal", 132),
    ("pickel", "block", "block_none", 2),
    ("pit", "shield", "shield_none", 8),
    ("pit", "shield", "shield_normal", 4),
    ("pit", "weapon", "weapon_arm_r", 4),
    ("pit", "weapon", "weapon_none", 10),
    ("pitb", "shield", "shield_none", 8),
    ("pitb", "shield", "shield_normal", 4),
    ("pitb", "weapon", "weapon_arm_r", 4),
    ("pitb", "weapon", "weapon_final", 4),
    ("pitb", "weapon", "weapon_none", 9),
    ("popo", "hammer", "hammer_invisible", 13),
    ("reflet", "book", "book_none", 1),
    ("reflet", "sword", "sword_thunder", 2),
    ("rockman", "lspark", "lspark_off", 1),
    ("rockman", "lspark", "lspark_on", 1),
    ("rockman", "rspark", "rspark_off", 1),
    ("rockman", "rspark", "rspark_on", 1),
    ("roy", "sword", "none", 2),
    ("roy", "sword", "sword_lhand", 2),
    ("roy", "sword", "sword_lhand_saya", 4),
    ("roy", "sword", "sword_rhand", 6),
    ("roy", "sword", "sword_rhand_saya", 2),
    ("roy", "sword", "sword_saya_tsuka", 8),
    ("roy", "sword", "sword_sword_saya", 6),
    ("samus", "body", "body_hide_gun", 29),
    ("samus", "body", "body_normal", 18),
    ("samus", "body", "body_sphere", 4),
    ("samusd", "body", "body_hide_gun", 28),
    ("samusd", "body", "body_normal", 17),
    ("samusd", "body", "body_sphere", 2),
    ("shizue", "item", "item_none", 2),
    ("shulk", "body", "body_monad_behind", 8),
    ("shulk", "body", "body_monad_hand", 87),
    ("sonic", "body", "body_normal", 84),
    ("sonic", "body", "body_sphere", 78),
    ("tantan", "arm_l", "arm_l_big", 2),
    ("tantan", "punch3_ring", "punch3_ring_hide", 4),
    ("tantan", "punch3_ring", "punch3_ring_normal", 2),
    ("tantan", "punch_r", "punch_r_1", 3),
    ("tantan", "punch_r", "punch_r_2", 2),
    ("tantan", "punch_r", "punch_r_3", 2),
    ("tantan", "shoulder_l", "shoulder_l_big", 2),
    ("toonlink", "shield", "shield_back", 143),
    ("toonlink", "shield", "shield_normal", 54),
    ("toonlink", "sword", "sword_back", 69),
    ("toonlink", "sword", "sword_normal", 30),
    ("trail", "sword", "sword_hide", 1),
    ("wolf", "blaster", "blaster_hide", 2),
    ("wolf", "blaster", "blaster_normal", 2),
    ("yoshi", "body", "body_egg", 12),
    ("yoshi", "body", "body_normal", 6),
    ("younglink", "shield", "shield_back", 134),
    ("younglink", "shield", "shield_normal", 54),
    ("younglink", "sword", "sword_back", 59),
    ("younglink", "sword", "sword_normal", 28),
];

/// Compatibility with existing saved scripts: this exact pair was supported before the
/// owner catalog. No other pair inherits its legacy context or owner permissions.
pub(crate) fn is_legacy_pair(group: &str, status: &str) -> bool {
    group == "body" && status == "body_normal"
}

pub(crate) fn is_measured_pair(group: &str, status: &str) -> bool {
    PINNED_VISIBILITY_PAIR_COUNTS
        .iter()
        .any(|(_, g, s, _)| *g == group && *s == status)
}

pub(crate) fn pairs_for(fighter: &str) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
    PINNED_VISIBILITY_PAIR_COUNTS
        .iter()
        .filter_map(move |(owner, group, status, _)| {
            owner
                .eq_ignore_ascii_case(fighter)
                .then_some((*group, *status))
        })
}

pub(crate) fn is_measured_for(fighter: &str, group: &str, status: &str) -> bool {
    PINNED_VISIBILITY_PAIR_COUNTS
        .iter()
        .any(|(owner, measured_group, measured_status, _)| {
            owner.eq_ignore_ascii_case(fighter)
                && *measured_group == group
                && *measured_status == status
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_visibility_catalog_is_unique_complete_pairs_not_a_cross_product() {
        let mut rows = std::collections::BTreeSet::new();
        let mut pairs = std::collections::BTreeSet::new();
        let mut owners = std::collections::BTreeSet::new();
        let mut total = 0;
        for &(owner, group, status, count) in PINNED_VISIBILITY_PAIR_COUNTS {
            assert!(rows.insert((owner, group, status)));
            pairs.insert((group, status));
            owners.insert(owner);
            total += count;
            assert!(count > 0);
            for name in [group, status] {
                assert!(!name.is_empty());
                assert!(name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
            }
        }
        assert_eq!(
            (rows.len(), pairs.len(), owners.len(), total),
            (180, 132, 50, 3_228)
        );
        assert!(is_measured_for("cloud", "sword", "sword_l"));
        assert!(!is_measured_for("kirby", "sword", "sword_l"));
        assert!(is_measured_for("kamui", "front_hair", "front_hair_hide"));
        assert!(!is_measured_for("kamui", "hair", "front_hair_hide"));
        assert!(pairs_for("cloud_copy").next().is_none());
        assert!(is_legacy_pair("body", "body_normal"));
        assert!(!is_legacy_pair("body", "body_sphere"));
    }
}
