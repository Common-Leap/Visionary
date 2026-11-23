//! Main executable offsets for the supported SSBU builds.

use std::sync::OnceLock;

#[derive(Clone, Copy)]
pub(crate) enum GameVersion {
    V13_0_4,
    V13_0_5,
}

impl GameVersion {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::V13_0_4 => "13.0.4",
            Self::V13_0_5 => "13.0.5",
        }
    }
}

static VERSION: OnceLock<Option<GameVersion>> = OnceLock::new();

/// The running game's version, for messages about it.
pub(crate) fn version_name() -> &'static str {
    VERSION
        .get()
        .copied()
        .flatten()
        .map_or("unknown", GameVersion::name)
}

/// Detect the game build before any offset-backed hooks are installed.
pub(crate) fn initialize() -> Option<GameVersion> {
    *VERSION.get_or_init(|| {
        let display = skyline::nn::oe::get_display_version();
        [GameVersion::V13_0_4, GameVersion::V13_0_5]
            .into_iter()
            .find(|version| version.name() == display.as_str())
    })
}

pub(crate) struct GameOffset(usize, usize);

impl GameOffset {
    pub(crate) fn get(&self) -> usize {
        match VERSION.get().copied().flatten().expect("unsupported SSBU version") {
            GameVersion::V13_0_4 => self.0,
            GameVersion::V13_0_5 => self.1,
        }
    }
}

// Columns are 13.0.4 and 13.0.5, relative to the main executable's text base.
// Sources: Smashline's effects, resources and fighter-name tables, and Clone Engine's
// game_reloc_13_0_5 text ranges and game.rs data relocation for the other entries.
pub(crate) const LOAD_EFFECTS_RESIDENT_INTERIOR: GameOffset = GameOffset(0x3563488, 0x3563a38);
pub(crate) const LOAD_EFFECTS_CALLER_RETURN: GameOffset = GameOffset(0x35636e8, 0x3563c98);
pub(crate) const RETAIN_FILE: GameOffset = GameOffset(0x3540450, 0x3540a00);
pub(crate) const RELEASE_FILE: GameOffset = GameOffset(0x3540560, 0x3540b10);
pub(crate) const FILESYSTEM_INFO: GameOffset = GameOffset(0x5331f20, 0x5332f20);
pub(crate) const ITEM_MANAGER: GameOffset = GameOffset(0x52c3070, 0x52c4070);
pub(crate) const EFFECT_MANAGER: GameOffset = GameOffset(0x5333920, 0x5334920);
pub(crate) const LOAD_EFFECTS: GameOffset = GameOffset(0x355f8f0, 0x355fea0);
pub(crate) const UNLOAD_EFFECTS: GameOffset = GameOffset(0x3563720, 0x3563cd0);
pub(crate) const DRAIN_LOAD_QUEUE: GameOffset = GameOffset(0x354a120, 0x354a6d0);
pub(crate) const MGR_MUTEX_LOCK: GameOffset = GameOffset(0x39c1410, 0x39c19c0);
pub(crate) const MGR_MUTEX_UNLOCK: GameOffset = GameOffset(0x39c1420, 0x39c19d0);
pub(crate) const BUILD_EFFECT_SET: GameOffset = GameOffset(0xa4d90, 0xa4d90);
pub(crate) const GAME_MALLOC: GameOffset = GameOffset(0x392dce0, 0x392e290);
pub(crate) const RESOLVE_KIND: GameOffset = GameOffset(0x2601110, 0x2601560);
pub(crate) const TEX_GUARD: GameOffset = GameOffset(0x93f10, 0x93f10);
pub(crate) const REQ_IMPL: GameOffset = GameOffset(0x44de70, 0x44de70);
pub(crate) const ENSURE_DIR_LOADED: GameOffset = GameOffset(0x35407a0, 0x3540d50);
pub(crate) const READINESS: GameOffset = GameOffset(0x987f0, 0x987f0);
pub(crate) const CREATE_UNHELD_ASSET_ITEM: GameOffset = GameOffset(0x2284310, 0x2284310);
pub(crate) const MOTION_RESOURCE_CACHE: GameOffset = GameOffset(0x52b7a30, 0x52b8a30);
pub(crate) const RENDER_MODEL_SLOT_240: GameOffset = GameOffset(0x35d4800, 0x35d4db0);
pub(crate) const RENDER_MODEL_SLOT_1B0: GameOffset = GameOffset(0x35d44d0, 0x35d4a80);
pub(crate) const RENDER_MODEL_SLOT_188: GameOffset = GameOffset(0x35d4300, 0x35d48b0);
pub(crate) const RENDER_MODEL_SLOT_180: GameOffset = GameOffset(0x35d3fd0, 0x35d4580);
pub(crate) const RENDER_MODEL_SLOT_1B8: GameOffset = GameOffset(0x35d44e0, 0x35d4a90);
pub(crate) const MOTION_MODULE_SLOT_4F0: GameOffset = GameOffset(0x49f650, 0x49f650);
pub(crate) const VISIBILITY_OUTPUT_VTABLE: GameOffset = GameOffset(0x5238590, 0x5239590);
pub(crate) const MOTION_HELPER_VTABLE: GameOffset = GameOffset(0x5238fa8, 0x5239fa8);
pub(crate) const REBUILD_MESH_VISIBILITY_CACHE: GameOffset = GameOffset(0x491e00, 0x491e00);
pub(crate) const CONNECT_ANIMATION_OUTPUT: GameOffset = GameOffset(0x498eb0, 0x498eb0);
pub(crate) const SET_SHARED_MOTION_RESOURCE: GameOffset = GameOffset(0x499480, 0x499480);
pub(crate) const END_OLD_MOTION: GameOffset = GameOffset(0x6dee80, 0x6dee80);
pub(crate) const BIND_ANIMATION_CONTROLLERS: GameOffset = GameOffset(0x6defa0, 0x6defa0);
pub(crate) const DESTROY_ANIMATION_CONTROLLERS: GameOffset = GameOffset(0x6dfb30, 0x6dfb30);
pub(crate) const BIND_SWING_RESOURCES: GameOffset = GameOffset(0x4a5630, 0x4a5630);
pub(crate) const REBUILD_VISIBILITY_MAPPINGS: GameOffset = GameOffset(0x4e2760, 0x4e2760);
pub(crate) const LIVE_MESH_MASKS: GameOffset = GameOffset(0x4e37a0, 0x4e37a0);
pub(crate) const ASSIGN_MESH_MASK: GameOffset = GameOffset(0x4e37b0, 0x4e37b0);
pub(crate) const DETACH_PHYSICS_CONTROLLERS: GameOffset = GameOffset(0x4a6110, 0x4a6110);
pub(crate) const RELEASE_WEAK: GameOffset = GameOffset(0x39c20c0, 0x39c2670);
pub(crate) const UNREGISTER_MOTION: GameOffset = GameOffset(0x496210, 0x496210);
pub(crate) const REGISTER_RENDER_MODEL: GameOffset = GameOffset(0x3559220, 0x35597d0);
pub(crate) const CREATE_RENDER_MODEL: GameOffset = GameOffset(0x35c22e0, 0x35c2890);
pub(crate) const CREATE_MESH_VISIBILITY_BUFFER: GameOffset = GameOffset(0x35d6f40, 0x35d74f0);
pub(crate) const UNREGISTER_RENDER_MODEL: GameOffset = GameOffset(0x3559340, 0x35598f0);
pub(crate) const SET_MESH_RENDER_OVERRIDE: GameOffset = GameOffset(0x35e23b0, 0x35e2960);
pub(crate) const REFRESH_VISIBILITY_GROUPS: GameOffset = GameOffset(0x4e3960, 0x4e3960);
pub(crate) const BIND_VISIBILITY_OUTPUT: GameOffset = GameOffset(0x3680960, 0x3680f10);
pub(crate) const QUEUE_DIRECTORY_RELEASE: GameOffset = GameOffset(0x353eff0, 0x353f5a0);
pub(crate) const FIGHTER_NAMES: GameOffset = GameOffset(0x4f80e20, 0x4f81e20);
pub(crate) const WEAPON_NAMES: GameOffset = GameOffset(0x5185bd0, 0x5186bd0);
pub(crate) const WEAPON_OWNER_NAMES: GameOffset = GameOffset(0x5188240, 0x5189240);
