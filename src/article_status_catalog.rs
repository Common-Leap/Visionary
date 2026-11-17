//! Exact source-only article/status/API tuples from the pinned move corpus.
//!
//! Counts are lexical source evidence, not placement or runtime-lifetime proof. Shared status
//! names are retained only in their measured source-owner tuples, never as registry aliases.
//! Nana's two Popo-article calls are intentionally not authorable owner bindings.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Preset {
    pub fighter: &'static str,
    pub article: &'static str,
    pub status: &'static str,
    pub if_present: bool,
    pub source_count: usize,
}

const fn p(
    fighter: &'static str,
    article: &'static str,
    status: &'static str,
    if_present: bool,
    source_count: usize,
) -> Preset {
    Preset {
        fighter,
        article,
        status,
        if_present,
        source_count,
    }
}

pub(crate) const PRESETS: &[Preset] = &[
    p(
        "inkling",
        "FIGHTER_INKLING_GENERATE_ARTICLE_MEGAPHONELASER",
        "WEAPON_INKLING_MEGAPHONELASER_STATUS_KIND_FIRE",
        false,
        2,
    ),
    p(
        "jack",
        "FIGHTER_JACK_GENERATE_ARTICLE_WIREROPE",
        "WEAPON_JACK_WIREROPE_STATUS_KIND_BACK",
        false,
        2,
    ),
    p(
        "jack",
        "FIGHTER_JACK_GENERATE_ARTICLE_WIREROPE",
        "WEAPON_JACK_WIREROPE_STATUS_KIND_EXTEND",
        false,
        2,
    ),
    p(
        "link",
        "FIGHTER_LINK_GENERATE_ARTICLE_BOW",
        "WN_LINK_BOW_STATUS_KIND_BACK",
        true,
        2,
    ),
    p(
        "link",
        "FIGHTER_LINK_GENERATE_ARTICLE_BOW",
        "WN_LINK_BOW_STATUS_KIND_HAVE",
        true,
        2,
    ),
    p(
        "master",
        "FIGHTER_MASTER_GENERATE_ARTICLE_SWORD",
        "WEAPON_MASTER_SWORD_STATUS_KIND_BACK",
        false,
        2,
    ),
    p(
        "master",
        "FIGHTER_MASTER_GENERATE_ARTICLE_SWORD",
        "WEAPON_MASTER_SWORD_STATUS_KIND_EXTEND",
        false,
        2,
    ),
    p(
        "pikmin",
        "FIGHTER_PIKMIN_GENERATE_ARTICLE_DOLFIN",
        "WEAPON_PIKMIN_DOLFIN_STATUS_KIND_START",
        false,
        2,
    ),
    p(
        "popo",
        "FIGHTER_POPO_GENERATE_ARTICLE_RUBBER",
        "WEAPON_POPO_RUBBER_STATUS_KIND_FALL",
        true,
        1,
    ),
    p(
        "popo",
        "FIGHTER_POPO_GENERATE_ARTICLE_RUBBER",
        "WEAPON_POPO_RUBBER_STATUS_KIND_REWIND",
        true,
        1,
    ),
    p(
        "samus",
        "FIGHTER_SAMUS_GENERATE_ARTICLE_GBEAM",
        "WEAPON_SAMUS_GBEAM_STATUS_KIND_REWIND",
        true,
        2,
    ),
    p(
        "samus",
        "FIGHTER_SAMUS_GENERATE_ARTICLE_GBEAM",
        "WEAPON_SAMUS_GBEAM_STATUS_KIND_SHOOT",
        false,
        1,
    ),
    p(
        "samusd",
        "FIGHTER_SAMUSD_GENERATE_ARTICLE_GBEAM",
        "WEAPON_SAMUS_GBEAM_STATUS_KIND_REWIND",
        true,
        2,
    ),
    p(
        "samusd",
        "FIGHTER_SAMUSD_GENERATE_ARTICLE_GBEAM",
        "WEAPON_SAMUS_GBEAM_STATUS_KIND_SHOOT",
        false,
        1,
    ),
    p(
        "sonic",
        "FIGHTER_SONIC_GENERATE_ARTICLE_SUPERSONIC",
        "WEAPON_SONIC_SUPERSONIC_STATUS_KIND_START",
        false,
        2,
    ),
    p(
        "szerosuit",
        "FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP2",
        "WEAPON_SZEROSUIT_WHIP2_STATUS_KIND_REWIND",
        true,
        2,
    ),
    p(
        "szerosuit",
        "FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP2",
        "WEAPON_SZEROSUIT_WHIP2_STATUS_KIND_SHOOT",
        false,
        1,
    ),
    p(
        "toonlink",
        "FIGHTER_TOONLINK_GENERATE_ARTICLE_HOOKSHOT",
        "WEAPON_TOONLINK_HOOKSHOT_STATUS_KIND_REWIND",
        true,
        5,
    ),
    p(
        "toonlink",
        "FIGHTER_TOONLINK_GENERATE_ARTICLE_HOOKSHOT",
        "WEAPON_TOONLINK_HOOKSHOT_STATUS_KIND_SHOOT",
        false,
        4,
    ),
    p(
        "younglink",
        "FIGHTER_YOUNGLINK_GENERATE_ARTICLE_HOOKSHOT",
        "WEAPON_TOONLINK_HOOKSHOT_STATUS_KIND_REWIND",
        true,
        5,
    ),
    p(
        "younglink",
        "FIGHTER_YOUNGLINK_GENERATE_ARTICLE_HOOKSHOT",
        "WEAPON_TOONLINK_HOOKSHOT_STATUS_KIND_SHOOT",
        false,
        4,
    ),
];

pub(crate) fn presets_for(fighter: &str) -> impl Iterator<Item = &'static Preset> + '_ {
    PRESETS
        .iter()
        .filter(move |preset| preset.fighter == fighter)
}

pub(crate) fn is_measured_call(article: &str, status: &str, if_present: bool) -> bool {
    PRESETS.iter().any(|preset| {
        preset.article == article && preset.status == status && preset.if_present == if_present
    })
}

pub(crate) fn is_measured_for(
    fighter: &str,
    article: &str,
    status: &str,
    if_present: bool,
) -> bool {
    presets_for(fighter).any(|preset| {
        preset.article == article && preset.status == status && preset.if_present == if_present
    })
}
