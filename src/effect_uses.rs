//! Which moves of a fighter spawn each of its effects. Read from the fighter's move scripts and
//! from the scripts of the objects those moves create: Mario's fireball draws
//! `mario_fb_bullet_l` from its own script, and the fireball comes from his neutral special.

use std::collections::HashMap;

/// One move that plays an effect.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EffectUse {
    /// The move script, as the dump names it ("AttackS4Lw"). `None` when the effect belongs
    /// to an object no move script creates (status code does).
    pub script: Option<String>,
    /// The object that plays the effect for the move, like "fireball".
    pub object: Option<String>,
    /// The bone the script spawns it on ("top", "haver").
    pub bone: Option<String>,
}

/// Effect name hash to the moves that play it.
pub type Uses = HashMap<u64, Vec<EffectUse>>;

const DUMP: &str = "https://api.github.com/repos/WuBoytH/SSBU-Dumped-Scripts/contents/smashline";

/// A listing from the dump, cached forever beside the scripts: the names of its files
/// (without `.txt`) or of its folders.
fn listing(path: &str, cache: &std::path::Path, dirs: bool) -> anyhow::Result<Vec<String>> {
    let body = match std::fs::read_to_string(cache) {
        Ok(body) => body,
        Err(_) => {
            let url = format!("{DUMP}/{path}?ref={}", crate::acmd::DUMP_REV);
            let body = crate::acmd::http_client()
                .get(&url)
                .header("User-Agent", "visionary")
                .send()?
                .text()?;
            // Only real listings are kept; the API answers errors with an object.
            if body.trim_start().starts_with('[') {
                if let Some(dir) = cache.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(cache, &body);
            }
            body
        }
    };
    let entries: Vec<serde_json::Value> = serde_json::from_str(&body)?;
    Ok(entries
        .iter()
        .filter(|e| (e.get("type").and_then(|t| t.as_str()) == Some("dir")) == dirs)
        .filter_map(|e| e.get("name").and_then(|n| n.as_str()))
        .map(|name| name.strip_suffix(".txt").unwrap_or(name).to_owned())
        .collect())
}

fn cache_dir(agent: &str) -> std::path::PathBuf {
    crate::scratch_dirs::app_storage_root()
        .join("script-cache")
        .join(agent)
}

/// Every script name (Pascal case) the dump has for one of the fighter's agents: the fighter
/// itself or one of its objects ("mario_fireball").
pub fn script_names(fighter: &str, agent: &str) -> anyhow::Result<Vec<String>> {
    listing(
        &format!("lua2cpp_{fighter}/{agent}"),
        &cache_dir(agent).join("_index.json"),
        false,
    )
}

/// The fighter's objects with scripts of their own, like "mario_fireball".
fn object_agents(fighter: &str) -> Vec<String> {
    listing(
        &format!("lua2cpp_{fighter}"),
        &cache_dir(fighter).join("_agents.json"),
        true,
    )
    .unwrap_or_default()
    .into_iter()
    .filter(|agent| agent != fighter)
    .collect()
}

/// The effects a script spawns, each with the bone it spawns it on: the first name of every
/// effect macro that makes one and the bone name after it (the third name of a flip macro,
/// whose second is the effect for the other facing), in every branch (a left-facing fighter's
/// `mario_fb_bullet_l` as well as the right's), sorted, once each.
///
/// A call is found with or without `macros::` in front: a moveset that glob-imports
/// `smash_script::macros::*` writes `EFFECT(agent, …)`, and the game's dump never does.
fn spawns(body: &str) -> Vec<(String, Option<String>)> {
    let is_name_char = |c: char| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_';
    let mut found: Vec<(String, Option<String>)> = body
        .lines()
        // Only the code of a line: a commented-out spawn plays nothing.
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .flat_map(|line| {
            line.match_indices('(').filter_map(move |(open, _)| {
                let before = &line[..open];
                let start = before
                    .char_indices()
                    .rev()
                    .find(|(_, c)| !is_name_char(*c))
                    .map_or(0, |(i, c)| i + c.len_utf8());
                let macro_name = &before[start..];
                // A whole upper-case name: `fooEFFECT(` or `x.EFFECT(` is not a macro call.
                let whole = before[..start]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '.'));
                let spawns = whole
                    && macro_name.starts_with(|c: char| c.is_ascii_uppercase())
                    && macro_name.contains("EFFECT")
                    && !["OFF", "DETACH", "REMOVE", "KILL", "SET_", "END"]
                        .iter()
                        .any(|word| macro_name.contains(word));
                if !spawns {
                    return None;
                }
                // The effect is the argument after the agent.
                let (_, rest) = line[open + 1..].split_once(',')?;
                let rest = rest.trim_start().strip_prefix("Hash40::new(\"")?;
                let (name, rest) = rest.split_once('"')?;
                // A flip macro names the effect for the other facing second and the bone third.
                let names_before_bone = if macro_name.contains("FLIP") { 2 } else { 1 };
                let bone = rest
                    .split("Hash40::new(\"")
                    .nth(names_before_bone)
                    .and_then(|rest| rest.split_once('"'))
                    .map(|(bone, _)| bone.to_owned());
                Some((name.to_owned(), bone))
            })
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

/// The uses a linked moveset's own scripts add, from [`crate::acmd_src::SourceIndex::
/// fighter_script_bodies`]: an effect a move script of the fighter's spawns is that move's
/// (`effect_attacklw3` is the move `attacklw3`), and one an object's script spawns is that
/// object's (`pacman_bigpacman` is `bigpacman`). Without these, every effect a moveset adds
/// read as one no move plays, since the game's scripts never name it.
pub fn project_uses(fighter: &str, scripts: &[(String, String, String)]) -> Uses {
    let mut uses = Uses::new();
    for (agent, script, body) in scripts {
        let object = (agent != fighter).then(|| {
            agent
                .strip_prefix(&format!("{fighter}_"))
                .unwrap_or(agent)
                .to_owned()
        });
        let script_name = object.is_none().then(|| {
            crate::acmd::SCRIPT_PREFIXES
                .iter()
                .find_map(|prefix| script.strip_prefix(prefix))
                .unwrap_or(script)
                .to_owned()
        });
        for (effect, bone) in spawns(body) {
            let found = EffectUse {
                script: script_name.clone(),
                object: object.clone(),
                bone,
            };
            let list = uses
                .entry(crate::app::effect_name_hash(&effect))
                .or_default();
            if !list.contains(&found) {
                list.push(found);
            }
        }
    }
    uses
}

/// The objects a script creates ("FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL" is "fireball").
fn created_objects(body: &str) -> Vec<String> {
    let mut names: Vec<String> = body
        .split("generate_article")
        .skip(1)
        .filter_map(|rest| {
            let args = rest.split(')').next()?;
            let symbol = args.split(',').nth(1)?.trim();
            crate::article_preview::article_name(symbol)
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Scans the fighter's scripts on a thread. Sends what is known so far as it goes, then the
/// whole index with `true` once every script is read.
pub fn spawn_scan(fighter: &str) -> std::sync::mpsc::Receiver<(Uses, bool)> {
    let (tx, rx) = std::sync::mpsc::channel();
    let fighter = fighter.to_owned();
    std::thread::spawn(move || {
        let moves = match script_names(&fighter, &fighter) {
            Ok(moves) => moves,
            Err(e) => {
                eprintln!("[EFFECT-USES] {fighter}: no move list: {e}");
                let _ = tx.send((Uses::new(), true));
                return;
            }
        };
        let bodies = fetch_all(&moves, |script| {
            crate::acmd::fetch_script_body_cached(&fighter, &pascal_to_snake(script)).ok()
        });
        let mut uses = Uses::new();
        let add = |uses: &mut Uses, effect: &str, found: EffectUse| {
            let list = uses
                .entry(crate::app::effect_name_hash(effect))
                .or_default();
            if !list.contains(&found) {
                list.push(found);
            }
        };
        let mut creators: HashMap<String, Vec<String>> = HashMap::new();
        for (script, body) in &bodies {
            for (effect, bone) in spawns(body) {
                add(
                    &mut uses,
                    &effect,
                    EffectUse {
                        script: Some(script.clone()),
                        object: None,
                        bone,
                    },
                );
            }
            for object in created_objects(body) {
                creators.entry(object).or_default().push(script.clone());
            }
        }
        if tx.send((uses.clone(), false)).is_err() {
            return;
        }
        for agent in object_agents(&fighter) {
            let object = agent
                .strip_prefix(&format!("{fighter}_"))
                .unwrap_or(&agent)
                .to_owned();
            let scripts = script_names(&fighter, &agent).unwrap_or_default();
            let bodies = fetch_all(&scripts, |script| {
                crate::acmd::fetch_object_script(&fighter, &agent, script)
            });
            let movers = creators.get(&object).cloned().unwrap_or_default();
            for (_, body) in &bodies {
                for (effect, bone) in spawns(body) {
                    if movers.is_empty() {
                        add(
                            &mut uses,
                            &effect,
                            EffectUse {
                                script: None,
                                object: Some(object.clone()),
                                bone: bone.clone(),
                            },
                        );
                    }
                    for mover in &movers {
                        add(
                            &mut uses,
                            &effect,
                            EffectUse {
                                script: Some(mover.clone()),
                                object: Some(object.clone()),
                                bone: bone.clone(),
                            },
                        );
                    }
                }
            }
        }
        let _ = tx.send((uses, true));
    });
    rx
}

/// Each name's script body, read by a few threads at once (most come from the disk cache).
fn fetch_all(
    names: &[String],
    fetch: impl Fn(&str) -> Option<String> + Sync,
) -> Vec<(String, String)> {
    let queue = std::sync::Mutex::new(names.to_vec());
    let found = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..crate::acmd::SCRIPT_FETCH_THREADS.min(names.len()) {
            scope.spawn(|| loop {
                let Some(name) = queue.lock().ok().and_then(|mut q| q.pop()) else {
                    return;
                };
                if let Some(body) = fetch(&name).filter(|body| !body.is_empty()) {
                    if let Ok(mut found) = found.lock() {
                        found.push((name, body));
                    }
                }
            });
        }
    });
    let mut found = found.into_inner().unwrap_or_default();
    found.sort();
    found
}

/// "AttackAirN" is "attack_air_n"; digits stay attached ("Win2" is "win2").
pub fn pascal_to_snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether a move (by motion name, "attack_s4_s") is the one a script (by dump name,
/// "AttackS4") belongs to. The names agree once underscores and case are gone, except that
/// some fighters' scripts drop the forward smash's and tilt's `s`.
pub fn script_is_for_motion(script: &str, motion: &str) -> bool {
    let motion = motion.replace('_', "").to_ascii_lowercase();
    let script = script.to_ascii_lowercase();
    crate::acmd::script_name_aliases(&format!("game_{motion}"))
        .iter()
        .any(|alias| alias.eq_ignore_ascii_case(&format!("game_{script}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_come_from_generate_article() {
        let body = "ArticleModule::generate_article(agent.module_accessor, \
                    *FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL, false, -1);\n\
                    ArticleModule::set_visibility_whole(agent.module_accessor, \
                    *FIGHTER_MARIO_GENERATE_ARTICLE_DOKAN, false, x);";
        assert_eq!(created_objects(body), vec!["fireball".to_owned()]);
    }

    #[test]
    fn bare_macro_calls_spawn_and_comments_do_not() {
        let body = "frame(agent.lua_state_agent, 3.0);\n\
                    if macros::is_excute(agent) {\n\
                    EFFECT(agent, Hash40::new(\"carrot_pull\"), Hash40::new(\"top\"), 0, 0, 6.8, 0, 0, 0, 0.8, 0, 0, 0, 0, 0, 0, true);\n\
                    LAST_EFFECT_SET_RATE(agent, 1.2);\n\
                    // EFFECT(agent, Hash40::new(\"old_dust\"), Hash40::new(\"top\"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true);\n\
                    EFFECT_OFF_KIND(agent, Hash40::new(\"carrot_trail\"), false, false);\n\
                    foo.EFFECT(agent, Hash40::new(\"method_call\"));\n\
                    }\n";
        assert_eq!(
            spawns(body),
            vec![("carrot_pull".to_owned(), Some("top".to_owned()))]
        );
    }

    #[test]
    fn project_scripts_credit_their_move_or_object() {
        let spawn = |name: &str| {
            format!("macros::EFFECT_FOLLOW(agent, Hash40::new(\"{name}\"), Hash40::new(\"top\"), 0, 0, 0, 0, 0, 0, 1, true);")
        };
        let scripts = vec![
            (
                "pacman".to_owned(),
                "effect_attacklw3".to_owned(),
                spawn("carrot_pull"),
            ),
            (
                "pacman".to_owned(),
                "game_attacklw3".to_owned(),
                spawn("carrot_pull"),
            ),
            (
                "pacman_bigpacman".to_owned(),
                "effect_specialnhop".to_owned(),
                spawn("fryguy_fire"),
            ),
        ];
        let uses = project_uses("pacman", &scripts);
        let carrot = &uses[&crate::app::effect_name_hash("carrot_pull")];
        assert_eq!(
            carrot,
            &vec![EffectUse {
                script: Some("attacklw3".to_owned()),
                object: None,
                bone: Some("top".to_owned()),
            }],
            "one use however many of the move's scripts spawn it"
        );
        assert!(script_is_for_motion(
            carrot[0].script.as_deref().unwrap(),
            "attack_lw3"
        ));
        assert_eq!(
            uses[&crate::app::effect_name_hash("fryguy_fire")],
            vec![EffectUse {
                script: None,
                object: Some("bigpacman".to_owned()),
                bone: Some("top".to_owned()),
            }]
        );
    }

    #[test]
    fn scripts_find_their_motions() {
        assert!(script_is_for_motion("AttackS4", "attack_s4_s"));
        assert!(script_is_for_motion("AttackS4Lw", "attack_s4_lw"));
        assert!(script_is_for_motion("Win2", "win_2"));
        assert!(script_is_for_motion("SpecialAirN", "special_air_n"));
        assert!(!script_is_for_motion("AttackS4", "attack_s4_lw"));
    }

    #[test]
    fn effects_come_from_every_branch_and_kills_do_not_count() {
        let body = "unsafe extern \"C\" fn game_x(agent: &mut L2CAgentBase) {\n}\n\
                    unsafe extern \"C\" fn effect_x(agent: &mut L2CAgentBase) {\n\
                    frame(agent.lua_state_agent, 3.0);\n\
                    if macros::is_excute(agent) {\n\
                    macros::EFFECT(agent, Hash40::new(\"mario_fb_shoot\"), Hash40::new(\"havel\"), 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, true);\n\
                    macros::EFFECT_OFF_KIND(agent, Hash40::new(\"sys_off\"), false, false);\n\
                    if PostureModule::lr(agent.module_accessor) < 0.0 {\n\
                    macros::EFFECT_FOLLOW(agent, Hash40::new(\"mario_fb_bullet_l\"), Hash40::new(\"rot\"), 0, 1.8, 0, 0, 0, 0, 1, true);\n\
                    }\n}\n";
        assert_eq!(
            spawns(body),
            vec![
                ("mario_fb_bullet_l".to_owned(), Some("rot".to_owned())),
                ("mario_fb_shoot".to_owned(), Some("havel".to_owned())),
            ]
        );
    }

    /// A flip macro names the effect twice, the second time for the other facing, and the bone
    /// third. Read as the second name, the bone of Diddy Kong's down tilt arc was the arc
    /// itself, no root bone, and the arc played on its own in the world's axes, a quarter turn
    /// off the way his move throws it. Verbatim from diddy/AttackLw3.
    #[test]
    fn a_flip_macro_spawns_on_its_third_name() {
        let body = "macros::EFFECT_FOLLOW_FLIP(agent, Hash40::new(\"diddy_attack_arc_c\"), Hash40::new(\"diddy_attack_arc_c\"), Hash40::new(\"top\"), -0.5, 5, 6, -9, 28, -158, 1, true, *EF_FLIP_YZ);";
        assert_eq!(
            spawns(body),
            vec![("diddy_attack_arc_c".to_owned(), Some("top".to_owned()))]
        );
    }
}
