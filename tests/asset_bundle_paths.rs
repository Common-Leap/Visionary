#[path = "../plugins/slight_replica/src/slight/effect_viewer/fighter_visibility.rs"]
mod fighter_visibility;

#[path = "../plugins/slight_replica/src/slight/effect_viewer/native_shared.rs"]
mod native_shared;

#[path = "../plugins/slight_replica/src/slight/effect_viewer/asset_identity.rs"]
mod asset_identity;

#[path = "../plugins/slight_replica/src/slight/effect_viewer/asset_path.rs"]
mod asset_path;

use asset_path::{validate_game_path, validate_payload_path, AssetFamily, PAYLOAD_PREFIX};

#[test]
fn plugin_allowlist_accepts_complete_fighter_asset_families() {
    let cases = [
        (
            "fighter/pickel/model/body/c07/model.numdlb",
            AssetFamily::Model,
        ),
        (
            "fighter/pickel/model/body/c07/model.nusktb",
            AssetFamily::Model,
        ),
        (
            "fighter/pickel/model/body/c07/albedo.nutexb",
            AssetFamily::Model,
        ),
        (
            "fighter/pickel/motion/body/c07/attack11.nuanmb",
            AssetFamily::Motion,
        ),
        (
            "fighter/pickel/model/body/c07/lod.xmb",
            AssetFamily::Model,
        ),
        (
            "fighter/pickel/motion/body/c07/swing.prc",
            AssetFamily::Motion,
        ),
        (
            "fighter/pickel/motion/body/c07/swingblend.prc",
            AssetFamily::Motion,
        ),
    ];
    for (path, family) in cases {
        assert_eq!(validate_game_path("pickel", path), Ok(family), "{path}");
        assert!(validate_payload_path(42, path, &format!("{PAYLOAD_PREFIX}42/{path}")).is_ok());
        assert!(validate_payload_path(43, path, &format!("{PAYLOAD_PREFIX}42/{path}")).is_err());
    }
}

#[test]
fn plugin_allowlist_rejects_noncanonical_or_dangerous_paths() {
    for path in [
        "fighter/pickel/model/body/c07/../c00/model.numdlb",
        "/fighter/pickel/model/body/c07/model.numdlb",
        "fighter\\pickel\\model\\body\\c07\\model.numdlb",
        "fighter/Pickel/model/body/c07/model.numdlb",
        "fighter/mario/model/body/c07/model.numdlb",
        "fighter/pickel/param/vl.prc",
        "fighter/pickel/motion/body/c07/update.prc",
        "effect/assist/bomberman/ef_bomberman.eff",
    ] {
        assert!(
            validate_game_path("pickel", path).is_err(),
            "accepted {path}"
        );
    }
}
