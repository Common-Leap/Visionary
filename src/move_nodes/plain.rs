//! The words a non-technical user sees. Internal block names stay stable for saved projects and
//! tests; everything shown on a card or in the palette goes through here.
use super::model::{MoveCondition, MoveNode, NativeComparison, NativeIntOperand, NodeKind};

/// What a block is called on screen, by its internal name.
pub fn block_name(internal: &str) -> &str {
    match internal {
        "Frame" | "At motion frame" | "Execute at frame" | "Native execute frame" => "At frame",
        "Wait for motion loop" => "Wait for animation loop",
        "Sync motion loop" => "Match animation loop",
        "Clear hitboxes" => "End all hitboxes",
        "Clear one hitbox" | "Clear hitbox" => "End one hitbox",
        "Create hitbox" => "Hitbox",
        "Reverse facing" => "Turn around",
        "Branch" => "If",
        "Motion rate" => "Animation speed",
        "Grab volume" | "Create grab" => "Grab box",
        "Clear grabs" => "End grab boxes",
        "Search volume" => "Detection box",
        "Hurtbox status" => "Invincible or intangible",
        "Reset hurtboxes" | "All hurtboxes normal" => "Hurtboxes back to normal",
        "Set velocity" => "Set speed",
        "Add velocity (no limit)" => "Add speed",
        "Clear velocity" => "Stop moving",
        "Set airborne" => "Go airborne",
        "Armor / damage reaction" => "Armor",
        "Modify hitbox power" => "Change hitbox power",
        "Reaction frames" | "Reaction frames · revised" => "Hit reaction time",
        "Wind area" => "Wind push",
        "Erase wind" => "End wind",
        "Define throw damage" => "Throw damage",
        "Define absolute attack" => "Fixed attack",
        "Apply throw" => "Throw opponent",
        "Native flag" | "Work flag" => "Game flag",
        "Set slowdown" => "Slow down time",
        "Clear slowdown" => "Back to normal time",
        "Slow opponents" => "Slow opponents",
        "Shield setoff multiplier (three attacks)"
        | "Shield setoff (three attacks)"
        | "Shield setoff multiplier (four attacks)"
        | "Shield setoff (four attacks)"
        | "Shield setoff multiplier" => "Shield pushback",
        "Notify event" => "Game notice",
        "Change status" => "Go to another action",
        "Allow transition" | "Allow transition group" => "Allow cancel",
        "Disable transition" | "Disable transition group" => "Block cancel",
        "Generate article" => "Spawn object",
        "Generate enabled article" => "Spawn object if allowed",
        "Article state" | "Article status" => "Object state",
        "Spawn article" => "Spawn object",
        "Remove article" => "Remove object",
        "Article visibility" => "Show or hide object",
        "Launch article" => "Launch object",
        "Launch if present" => "Launch object if out",
        "Article motion" => "Object animation",
        "Send event" => "Tell a gimmick",
        "Effect spawn" | "Spawn effect" => "Visual effect",
        "Follow native effect" => "Effect that follows the fighter",
        "Slope" | "Slope alignment" => "Lean with the ground",
        "Effect off" | "Native effect off" => "End visual effect",
        "Imported effect" => "Visual effect",
        "Rumble pattern" => "Rumble",
        "Attack module set no damage fly smoke all" => "No launch smoke",
        "Attack module init attack pos" => "Reset hitbox positions",
        "Attack module set final finish cut in" => "Final Smash finish cut-in",
        "Attack module set height all" => "Hitbox height",
        "Attack module set all" | "Attack module set indexed" => "Hitbox setting",
        "Attack fp" => "Hitbox at a point",
        "Grab rebound" => "Grab bounce-off",
        "Item visibility" => "Show or hide held item",
        "Jostle set status" => "Push other fighters",
        "Kinetic add speed" | "Kinetic energy" => "Change speed",
        "Kinetic clear speed all" => "Stop moving",
        "Kinetic set consider ground friction" => "Ground friction",
        "Change kinetic" => "Movement type",
        "Clr speed" => "Stop moving",
        "Col pri" | "Col normal" => "Fighter tint",
        "Correct" => "Ground snap",
        "Damage no reaction" => "Armor",
        "Ft catch stop" => "Stop at ledge",
        "Ft start adjust motion frame" => "Adjust animation start",
        "Motion module set frame partial" => "Part animation frame",
        "Motion module set helper calculation" => "Animation helper",
        "Motion module set rate" => "Animation speed",
        "Motion module set rate partial" => "Part animation speed",
        "Set speed ex" => "Set speed",
        "Work module inc int" => "Count up a game value",
        "Gimmick event" => "Tell a gimmick",
        "Hit rumble" => "Rumble on hit",
        "Default catch setup" => "Standard grab setup",
        "Body collision" | "Body collision area" | "Body collision area XY" => "Push box",
        "Imported actions" => "Actions",
        "Imported effects" => "Visual effects",
        "Top-level command" => "Action",
        "Native structure" => "Original game code",
        "Native setter" | "Native call" | "Native slowdown" => "Game code",
        "New controller" => "New gimmick",
        "Controller" => "Gimmick",
        "Event receiver" => "When a move tells it",
        "Variable" => "Value",
        "Set value" => "Set a value",
        "Add to variable" => "Add to a value",
        "Emit event" => "Tell another rule",
        "Event source" => "When",
        "Entry state" => "First state",
        "Attack mod" => "Change hitbox power",
        "Article part visibility" => "Show or hide part of an object",
        "Attack catch-only mode" => "Hitbox only grabs",
        "Attack finish camera" => "Finishing-hit camera",
        "Attack height" => "Hitbox height",
        "Attack no-dead mode" => "Hitbox can't KO",
        "Attack reference joint" => "Hitbox follows a body part",
        "Attack target filter" => "What the hitbox can hit",
        "Cliff hang profile" => "Ledge grab range",
        "Damage fly smoke" => "Launch smoke",
        "Damage shake scale" => "Hit shake amount",
        "Disable attack tip" => "No hitbox tip effect",
        "Disable effect render offset" | "Disable render offset" => "Effect stays in place",
        "Enable safe position" => "Remember a safe spot",
        "Force attack reaction" => "Force hit reaction",
        "Inherited open motion" => "Keep the open pose",
        "Initialize attack position" => "Reset hitbox positions",
        "Interpolate attacks" => "Smooth hitbox movement",
        "Model default" => "Model back to normal",
        "Top-level sound" => "Sound",
        "Unique attack effects" => "Hit sparks",
        "Vector target" => "Pull opponents to a point",
        "Sync effect initial position" | "Sync initial position" => {
            "Effect starts where the fighter is"
        }
        "Circle post effect" => "Circle screen effect",
        "Remove line post effect" => "End line screen effect",
        "Fill screen model color" => "Fill the screen with color",
        "Final wave angle" => "Final Smash wave angle",
        "Item pull offset limit" => "Item pull limit",
        "Slope status" => "Lean with the ground",
        "Default visibility status" => "Model back to normal",
        "Stop status sounds" => "Stop this action's sounds",
        "Counter" => "Number",
        "Gauge" => "Meter",
        other => other,
    }
}

/// Palette section names.
pub fn group_name(internal: &str) -> &str {
    match internal {
        "Flow" => "Timing & logic",
        "Native gimmick" => "This fighter's values",
        "Character" => "Gimmick",
        other => other,
    }
}

/// Names of the four lanes and palette tabs.
pub const LANES: [&str; 4] = ["Action", "Visuals", "Sound", "Rumble & camera"];

/// One sentence on what a block does, by its on-screen name.
pub fn block_help(name: &str) -> Option<&'static str> {
    Some(match name {
        "At frame" => "Everything after this waits until the animation reaches this frame.",
        "Wait" => "Pauses for this many frames before the next block runs.",
        "When" => "Runs its blocks once the move reaches this point. After runs next either way.",
        "If" => "Checks something during the move. Yes runs when it is true, No when it is not, and After runs either way.",
        "Repeat" => "Runs the blocks in Loop several times.",
        "Hitbox" => "An area that hits opponents. It stays on until an End hitbox block.",
        "End all hitboxes" => "Stops every hitbox this move has turned on.",
        "End one hitbox" => "Stops one hitbox and leaves the others on.",
        "Grab box" => "An area that grabs opponents during a grab move. To grab from any other move, use Go to another action and pick catch.",
        "End grab boxes" => "Stops every grab box.",
        "Detection box" => "An area that notices opponents without hurting them.",
        "Invincible or intangible" => "Changes whether this fighter can be hit.",
        "Hurtboxes back to normal" => "This fighter can be hit as usual again.",
        "Armor" => "Lets this fighter take hits without flinching.",
        "Set speed" => "Sets how fast the fighter is moving.",
        "Add speed" => "Pushes the fighter on top of their current speed.",
        "Stop moving" => "Removes the fighter's current speed.",
        "Go airborne" => "Puts the fighter in the air.",
        "Turn around" => "Flips the way the fighter is facing.",
        "Animation speed" => "Stretches the animation. 1 is normal, 2 takes twice as long, 0.5 takes half as long.",
        "Slow down time" => "Slows the whole match. The game only does this during a Final Smash.",
        "Slow opponents" => "Slows everyone else. The game only does this during a Final Smash.",
        "Back to normal time" => "Ends Slow down time.",
        "Shield pushback" => "Changes how far these hitboxes push a shield back.",
        "Spawn object" => "Creates a projectile or other object that belongs to this fighter.",
        "Remove object" => "Removes this fighter's object.",
        "Throw damage" => "Sets the damage and launch of a throw.",
        "Throw opponent" => "Throws the grabbed opponent using Throw damage.",
        "Go to another action" => "Ends this move and switches the fighter to another action.",
        "Allow cancel" => "Lets the player leave this move early with the chosen action.",
        "Block cancel" => "Stops the player leaving this move with the chosen action.",
        "Tell a gimmick" => "Sends a message to one of this fighter's gimmicks.",
        "Visual effect" => "Shows a particle effect.",
        "End visual effect" => "Stops a particle effect.",
        "Play sound" => "Plays a sound.",
        "Stop sound" => "Stops a sound.",
        "Rumble" | "Rumble on hit" => "Shakes the controller.",
        "Camera shake" => "Shakes the screen.",
        "Original game code" => "Part of the game's own move kept exactly as it is.",
        "New gimmick" => "Adds a set of rules that run all the time for this fighter.",
        _ => return None,
    })
}

pub fn compare(op: NativeComparison) -> &'static str {
    match op {
        NativeComparison::Gte => "is at least",
        NativeComparison::Lte => "is at most",
        NativeComparison::Ne => "is not",
        NativeComparison::Gt => "is more than",
        NativeComparison::Lt => "is less than",
        NativeComparison::Eq => "is",
    }
}

/// The last part of a game symbol, in lower-case words.
pub fn symbol_words(symbol: &str) -> String {
    let symbol = symbol.trim_start_matches('*');
    if let Some(label) = crate::gimmicks::native_catalog::label_for(symbol) {
        return label.to_lowercase();
    }
    let tail = [
        "WORK_ID_",
        "_WORK_INT_",
        "_WORK_FLOAT_",
        "_WORK_FLAG_",
        "_FLAG_",
        "_STATUS_",
        "_GENERATE_ARTICLE_",
    ]
    .iter()
    .find_map(|separator| symbol.split_once(separator).map(|(_, tail)| tail))
    .unwrap_or(symbol);
    tail.trim_start_matches("INT_")
        .trim_start_matches("FLOAT_")
        .trim_start_matches("FLAG_")
        .replace('_', " ")
        .to_lowercase()
}

/// A game flag as a phrase that can follow "If".
fn flag_phrase(symbol: &str, expected: bool) -> String {
    let positive = match symbol {
        "FIGHTER_STATUS_ATTACK_FLAG_SMASH_SMASH_HOLD_TO_ATTACK" => Some((
            "the smash attack has been let go",
            "the smash attack has not been let go",
        )),
        "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO"
        | "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO_INPUT" => Some((
            "the next combo hit is available",
            "the next combo hit is not available",
        )),
        "FIGHTER_STATUS_ATTACK_FLAG_ENABLE_100" => {
            Some(("rapid jab is available", "rapid jab is not available"))
        }
        "FIGHTER_STATUS_ATTACK_AIR_FLAG_ENABLE_LANDING" => {
            Some(("landing is allowed", "landing is not allowed"))
        }
        "FIGHTER_STATUS_THROW_FLAG_START_AIR" => Some((
            "the throw started in the air",
            "the throw started on the ground",
        )),
        "FIGHTER_INSTANCE_WORK_ID_FLAG_RABBIT_CAP" => {
            Some(("wearing the Rabbit Hood", "not wearing the Rabbit Hood"))
        }
        _ => None,
    };
    match positive {
        Some((yes, no)) => (if expected { yes } else { no }).into(),
        None => format!(
            "\"{}\" is {}",
            move_words(&symbol_words(symbol)),
            if expected { "on" } else { "off" }
        ),
    }
}

/// What a check that is not really a choice marks, as words that read after "When". The game
/// sets the smash "hold to attack" flag when the smash is let go, charged or not, so its blocks
/// always run; charging only raises the damage.
pub fn release_point(condition: &MoveCondition) -> Option<&'static str> {
    let flag = match condition {
        MoveCondition::WorkFlag {
            symbol,
            expected: true,
        } => symbol.as_str(),
        MoveCondition::Source { header } if !header.contains('!') => header.as_str(),
        _ => return None,
    };
    flag.contains("FIGHTER_STATUS_ATTACK_FLAG_SMASH_SMASH_HOLD_TO_ATTACK")
        .then_some("the smash attack is let go (charged or not)")
}

/// A costume check: the game numbers costumes from 0, players from 1.
fn costume_words(op: NativeComparison, slot: i64) -> String {
    match op {
        NativeComparison::Eq => format!("the fighter wears costume {} (c{slot:02})", slot + 1),
        NativeComparison::Ne => format!(
            "the fighter does not wear costume {} (c{slot:02})",
            slot + 1
        ),
        op => format!("the costume number {} {}", compare(op), slot + 1),
    }
}

/// A game value such as `*FIGHTER_PICKEL_MATERIAL_KIND_GOLD` as the word a player uses ("gold").
fn value_words(value: &str) -> String {
    let value = value.trim().trim_start_matches('*');
    if value.parse::<f64>().is_ok() {
        return value.to_string();
    }
    let tail = ["_KIND_", "_STRENGTH_", "_SAVING_LV_", "_LV_"]
        .iter()
        .find_map(|separator| {
            value
                .rsplit_once(separator)
                .map(|(_, tail)| (separator.trim_matches('_'), tail))
        });
    match tail {
        Some(("STRENGTH", "W")) => "light".into(),
        Some(("STRENGTH", "M")) => "medium".into(),
        Some(("STRENGTH", "S")) => "heavy".into(),
        Some(("SAVING_LV" | "LV", level)) => format!("level {level}"),
        Some((_, tail)) => tail.replace('_', " ").to_lowercase(),
        None => move_words(&symbol_words(value)).trim().to_string(),
    }
}

/// "piece is out" as "piece is not out".
fn negate(words: &str) -> String {
    for (from, to) in [
        (" is out", " is not out"),
        (" is on", " is off"),
        (" is off", " is on"),
        (" wears ", " does not wear "),
    ] {
        if let Some(index) = words.rfind(from) {
            return format!("{}{to}{}", &words[..index], &words[index + from.len()..]);
        }
    }
    format!("it is not true that {words}")
}

/// A condition as words that read after "If".
pub fn condition_sentence(condition: &MoveCondition) -> String {
    match condition {
        MoveCondition::IsExecute => "the move is running".into(),
        MoveCondition::Source { header } => {
            let header = header.trim().trim_start_matches('}').trim();
            let header = header
                .trim_start_matches("else")
                .trim()
                .trim_start_matches("if")
                .trim()
                .trim_start_matches('{')
                .trim();
            if header.is_empty() {
                "none of the above".into()
            } else {
                condition_words(header)
            }
        }
        MoveCondition::CharacterValue {
            chart_id,
            var,
            op,
            value,
        } => {
            let right = match value {
                super::model::CharacterValue::Int(value) => value.to_string(),
                super::model::CharacterValue::Float(value) => value.to_string(),
                super::model::CharacterValue::Flag(value) => {
                    return format!("{var} is {}", if *value { "on" } else { "off" })
                }
            };
            let owner = if chart_id == "main" {
                String::new()
            } else {
                format!("{chart_id}'s ")
            };
            format!("{owner}{var} {} {right}", compare(*op))
        }
        MoveCondition::Grounded => "on the ground".into(),
        MoveCondition::Airborne => "in the air".into(),
        MoveCondition::MotionChanging => "the animation just changed".into(),
        MoveCondition::NativeLrNegative => "facing left".into(),
        MoveCondition::ItemHeld => "holding an item".into(),
        MoveCondition::InStatus { symbol } => {
            format!(
                "doing {}",
                crate::gimmicks::field_canvas::status_name(symbol)
            )
        }
        MoveCondition::HoldingItemKind { item } => {
            format!("holding {}", crate::gimmicks::graph_view::item_name(item))
        }
        MoveCondition::ArticleExists { symbol } => {
            format!("{} is out", object_name(symbol).to_lowercase())
        }
        MoveCondition::Stick {
            axis, op, value, ..
        } => format!(
            "the stick's {} {} {value}",
            match axis {
                super::model::StickAxis::X => "left-right position",
                super::model::StickAxis::Y => "up-down position",
            },
            compare(*op),
        ),
        MoveCondition::ButtonHeld { button } => {
            format!("the {button:?} button is held").to_lowercase()
        }
        MoveCondition::AttackInfliction { mask, .. } => match mask {
            super::model::InflictionMask::Hit => "this attack hit someone".into(),
            super::model::InflictionMask::Shield => "this attack hit a shield".into(),
        },
        MoveCondition::WorkFlag { symbol, expected } => flag_phrase(symbol, *expected),
        MoveCondition::WorkInt {
            symbol,
            op: NativeComparison::Eq,
            value: NativeIntOperand::FighterKind(fighter),
        } if symbol == "FIGHTER_INSTANCE_WORK_ID_INT_KIND" => {
            if fighter == "kirby" {
                "Kirby is using this move as a copy ability".into()
            } else {
                format!("the fighter is {fighter}")
            }
        }
        MoveCondition::WorkInt {
            symbol,
            op,
            value: NativeIntOperand::Literal(slot),
        } if symbol == "FIGHTER_INSTANCE_WORK_ID_INT_COLOR" => costume_words(*op, i64::from(*slot)),
        MoveCondition::WorkInt { symbol, op, value } => {
            let right = match value {
                NativeIntOperand::Literal(value) => value.to_string(),
                NativeIntOperand::FighterKind(fighter) => fighter.clone(),
                NativeIntOperand::Choice(symbol) => symbol
                    .split_once("_COSTUME_KIND_")
                    .map_or(symbol.as_str(), |(_, name)| name)
                    .replace('_', " ")
                    .to_lowercase(),
            };
            format!(
                "\"{}\" {} {right}",
                move_words(&symbol_words(symbol)),
                compare(*op)
            )
        }
        MoveCondition::WorkFloat { symbol, op, value } => {
            format!(
                "\"{}\" {} {value}",
                move_words(&symbol_words(symbol)),
                compare(*op)
            )
        }
        MoveCondition::And { .. } => "all of these are true".into(),
        MoveCondition::Or { .. } => "any of these is true".into(),
        MoveCondition::Not(inner) => negate(&condition_sentence(inner)),
        MoveCondition::Group(inner) => condition_sentence(inner),
    }
}

/// Removes wording that only meant something to the exporter.
pub fn detail(text: String) -> String {
    let cleaned = text
        .replace(" · Source export", "")
        .replace(" · source export", "")
        .replace("Own Execute guard", "")
        .replace("Native feedback", "");
    // Raw game names in a detail read as words.
    let cleaned = cleaned
        .split(" · ")
        .map(|part| {
            if part.contains("Hash40::") || part.starts_with('*') || part.starts_with("0x") {
                arg_words(part)
            } else {
                part.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" · ");
    match cleaned.as_str() {
        "Source export" | "source export" => String::new(),
        "Preserved source" => "Kept as the game wrote it".into(),
        "Runs outside Execute" => "Runs every time this point is reached".into(),
        "Preserved · not fully editable" => "Kept as the game wrote it".into(),
        _ => cleaned,
    }
}

/// Words the game glues together in object names.
const OBJECT_WORDS: &[&str] = &[
    "wicked", "weave", "arm", "leg", "fire", "ball", "shot", "bullet", "bomb", "beam", "sword",
    "missile", "star", "laser", "hammer", "shield", "pillar", "flame", "water", "stream", "dragon",
    "hand", "wing", "head", "chest", "ship", "knife", "aura", "shadow", "ice", "wind", "dummy",
    "object", "final", "charge", "gun", "bow", "arrow", "homing", "target", "hook", "whip", "wire",
    "rope", "power", "cursor", "reticle", "buddy", "rush", "coil", "metal", "blade", "hard",
    "knuckle", "leaf", "cutter", "death", "black", "hole", "thunder", "bird", "base", "barrel",
    "jet", "flower", "pot", "cannon", "spike", "spit", "stealth", "super", "crash", "tree", "big",
];

fn split_glued(word: &str) -> String {
    let mut rest = word;
    let mut parts = Vec::new();
    while !rest.is_empty() {
        let Some(found) = OBJECT_WORDS
            .iter()
            .filter(|part| rest.starts_with(**part))
            .max_by_key(|part| part.len())
        else {
            return word.into();
        };
        parts.push(*found);
        rest = &rest[found.len()..];
    }
    parts.join(" ")
}

/// "FIGHTER_BAYONETTA_GENERATE_ARTICLE_WICKEDWEAVELEG" becomes "Wicked weave leg".
pub fn object_name(symbol: &str) -> String {
    let words = symbol_words(symbol);
    let joined = words
        .split(' ')
        .map(split_glued)
        .collect::<Vec<_>>()
        .join(" ");
    let mut letters = joined.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}

/// The object a new spawn block starts on: something this fighter actually shoots, when it has
/// one, rather than whatever the game happens to list first (Mario's first object is Cappy).
pub fn default_object(fighter: &str) -> String {
    const SHOTS: [&str; 12] = [
        "FIREBALL",
        "SHOT",
        "BULLET",
        "BEAM",
        "ARROW",
        "MISSILE",
        "LASER",
        "BALL",
        "SHURIKEN",
        "BOOMERANG",
        "KNIFE",
        "BOMB",
    ];
    let objects: Vec<_> = crate::gimmicks::registry::entries_for(
        fighter,
        crate::gimmicks::registry::IdentifierKind::Article,
    )
    .map(|entry| entry.symbol)
    .collect();
    SHOTS
        .iter()
        .find_map(|shot| objects.iter().find(|symbol| symbol.contains(shot)))
        .or(objects.first())
        .map_or_else(String::new, |symbol| (*symbol).to_owned())
}

/// One whole-move starter, so a user can begin from a working shape instead of a vanilla script.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Starter {
    Blank,
    QuickStrike,
    HeavyHit,
    MultiHit,
    Lunge,
    Shoot,
}

impl Starter {
    pub const ALL: [Self; 6] = [
        Self::QuickStrike,
        Self::HeavyHit,
        Self::MultiHit,
        Self::Lunge,
        Self::Shoot,
        Self::Blank,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Blank => "Empty move",
            Self::QuickStrike => "Quick strike",
            Self::HeavyHit => "Heavy hit",
            Self::MultiHit => "Three quick hits",
            Self::Lunge => "Lunge forward and hit",
            Self::Shoot => "Shoot a projectile",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Self::Blank => "Removes every block so you can build from nothing. Undo brings them back.",
            Self::QuickStrike => "A fast, light hit. Replaces this move's blocks; Undo brings them back.",
            Self::HeavyHit => "A slow, strong hit. Replaces this move's blocks; Undo brings them back.",
            Self::MultiHit => "Three small hits in a row. Replaces this move's blocks; Undo brings them back.",
            Self::Lunge => "Moves forward, then hits. Replaces this move's blocks; Undo brings them back.",
            Self::Shoot => "Fires this fighter's first object. Replaces this move's blocks; Undo brings them back.",
        }
    }
}

fn hitbox(damage: f32, size: f32) -> NodeKind {
    let mut attack = crate::data::Hitbox::default().to_attack_call();
    attack.damage = damage;
    attack.size = size;
    // In front of the fighter at chest height, where an opponent is.
    attack.offset_y = 8.0;
    attack.offset_z = 10.0;
    NodeKind::Execute {
        commands: vec![crate::data::ExcuteStmt::Attack(attack)],
    }
}

/// The action blocks of a starter. `first_id` numbers them; `shot` is the fighter's spawn block.
pub fn starter_nodes(
    starter: Starter,
    first_id: u32,
    speed: NodeKind,
    shot: Option<NodeKind>,
) -> Vec<MoveNode> {
    let end = || NodeKind::Execute {
        commands: vec![crate::data::ExcuteStmt::ClearAll],
    };
    let frame = |frame| NodeKind::Frame { frame };
    let wait = |frames| NodeKind::Wait { frames };
    let kinds: Vec<NodeKind> = match starter {
        Starter::Blank => Vec::new(),
        Starter::QuickStrike => vec![frame(5.0), hitbox(6.0, 4.0), wait(3.0), end()],
        Starter::HeavyHit => vec![frame(14.0), hitbox(18.0, 6.0), wait(4.0), end()],
        Starter::MultiHit => vec![
            frame(5.0),
            hitbox(3.0, 4.0),
            wait(2.0),
            end(),
            wait(3.0),
            hitbox(3.0, 4.0),
            wait(2.0),
            end(),
            wait(3.0),
            hitbox(5.0, 5.0),
            wait(2.0),
            end(),
        ],
        Starter::Lunge => vec![
            frame(4.0),
            speed,
            wait(2.0),
            hitbox(10.0, 5.0),
            wait(4.0),
            end(),
        ],
        Starter::Shoot => shot.map_or_else(Vec::new, |shot| vec![frame(10.0), shot]),
    };
    kinds
        .into_iter()
        .zip(first_id..)
        .map(|(kind, id)| MoveNode { id, kind })
        .collect()
}

/// One readable line of the move, in running order.
pub struct StoryLine {
    pub frame: Option<f32>,
    pub depth: usize,
    pub text: String,
    pub node: Option<u32>,
}

/// The game's "if the move is running" wrapper, however it was read.
fn is_running_check(condition: &MoveCondition) -> bool {
    match condition {
        MoveCondition::IsExecute => true,
        MoveCondition::Source { header } => {
            let header = header
                .trim()
                .trim_start_matches("if")
                .trim()
                .trim_end_matches('{');
            condition_words(header) == "the move is running"
        }
        _ => false,
    }
}

/// Reads a script top to bottom as sentences. `caption` gives a block its name and detail.
pub fn story(
    nodes: &[MoveNode],
    depth: usize,
    frame: &mut f32,
    caption: &dyn Fn(&MoveNode) -> String,
    out: &mut Vec<StoryLine>,
) {
    for node in nodes {
        let mut line = |text: String, at: Option<f32>, depth: usize, id: Option<u32>| {
            out.push(StoryLine {
                frame: at,
                depth,
                text,
                node: id,
            })
        };
        match &node.kind {
            NodeKind::Frame { frame: at } | NodeKind::NativeExecuteFrame { frame: at } => {
                *frame = frame.max(*at)
            }
            NodeKind::Wait { frames } => *frame += frames.max(0.0),
            // The game wraps nearly everything in "if the move is running"; a reader
            // gains nothing from it.
            NodeKind::Branch {
                condition,
                then_nodes,
                else_nodes,
            } if is_running_check(condition) => {
                let start = *frame;
                story(then_nodes, depth, frame, caption, out);
                // The check is false only while the game skips ahead through the move.
                if !else_nodes.is_empty() {
                    out.push(StoryLine {
                        frame: None,
                        depth,
                        text: "When the game skips ahead past this point:".into(),
                        node: Some(node.id),
                    });
                    let mut other = start;
                    story(else_nodes, depth + 1, &mut other, caption, out);
                }
            }
            NodeKind::Branch {
                then_nodes,
                else_nodes,
                ..
            } => {
                line(
                    format!("{}:", caption(node)),
                    Some(*frame),
                    depth,
                    Some(node.id),
                );
                let start = *frame;
                let mut other = start;
                story(then_nodes, depth + 1, frame, caption, out);
                if !else_nodes.is_empty() {
                    out.push(StoryLine {
                        frame: None,
                        depth,
                        text: "Otherwise:".into(),
                        node: None,
                    });
                    story(else_nodes, depth + 1, &mut other, caption, out);
                }
                *frame = frame.max(other);
            }
            NodeKind::Repeat { body, count, .. } => {
                line(
                    format!("{}:", caption(node)),
                    Some(*frame),
                    depth,
                    Some(node.id),
                );
                let start = *frame;
                story(body, depth + 1, frame, caption, out);
                *frame = start + (*frame - start) * (*count).max(1) as f32;
            }
            _ => line(caption(node), Some(*frame), depth, Some(node.id)),
        }
    }
}

/// True when a script turns a hitbox on and never ends it, the classic beginner surprise.
pub fn leaves_hitbox_on(nodes: &[MoveNode]) -> bool {
    use crate::data::ExcuteStmt;
    let mut on = false;
    for node in nodes {
        if let NodeKind::Execute { commands } = &node.kind {
            for command in commands {
                match command {
                    ExcuteStmt::Attack(_) => on = true,
                    ExcuteStmt::ClearAll | ExcuteStmt::Clear(_) => on = false,
                    _ => {}
                }
            }
        }
    }
    on
}

/// Splits a call's argument list at top-level commas.
fn split_args(inside: &str) -> Vec<String> {
    let (mut depth, mut quoted, mut start, mut out) = (0i32, false, 0, Vec::new());
    for (index, character) in inside.char_indices() {
        match character {
            '"' => quoted = !quoted,
            '(' | '[' | '{' if !quoted => depth += 1,
            ')' | ']' | '}' if !quoted => depth -= 1,
            ',' if depth == 0 && !quoted => {
                out.push(inside[start..index].trim().to_owned());
                start = index + 1;
            }
            _ => {}
        }
    }
    let last = inside[start..].trim();
    if !last.is_empty() {
        out.push(last.to_owned());
    }
    out
}

/// "special_hi" style parts of game names, as words a player uses.
fn move_words(words: &str) -> String {
    let mut text = format!(" {} ", words.to_lowercase().replace('_', " "));
    for (from, to) in [
        (" special hi ", " up special "),
        (" special lw ", " down special "),
        (" special s ", " side special "),
        (" special n ", " neutral special "),
        (" special air hi ", " air up special "),
        (" special air lw ", " air down special "),
        (" attack air ", " air attack "),
        (" attack hi4 ", " up smash "),
        (" attack lw4 ", " down smash "),
        (" attack s4 ", " side smash "),
        (" attack hi3 ", " up tilt "),
        (" attack lw3 ", " down tilt "),
        (" attack s3 ", " side tilt "),
        (" lua script line max ", " arm fully stretched "),
        (" neutral special neutral special ", " neutral special "),
        (" special common int strength ", " attack strength "),
        (" saving lv ", " charge level "),
        (" int ", " "),
        (" flag ", " "),
        (" lw ", " down "),
        (" hi ", " up "),
        (" lr ", " facing "),
        (" mot ", " animation "),
        (" situation ", " ground or air "),
    ] {
        while text.contains(from) {
            text = text.replace(from, to);
        }
    }
    // The game names its most used switches in shorthand.
    match text.trim() {
        "sscope shoot" => "Super Scope fires",
        "fflower shoot" => "Fire Flower fires",
        "sscope aerial req sound" => "Super Scope air sound",
        "sscope aerial req effect" => "Super Scope air effect",
        "enable landing" => "can land",
        "reserve change status dlay motion" => "change action when the animation ends",
        "start smash hold" => "smash charge starts",
        "100 continue check" => "rapid jab keeps going",
        "enable 100" => "rapid jab allowed",
        "enable combo" => "next jab allowed",
        "enable no hit combo" => "next jab allowed without a hit",
        "backshield invisible" => "back shield hidden",
        "catch wait" => "holding the grab",
        "turn run" => "turning while running",
        "turn dash" => "turning while dashing",
        "final hit cancel" => "Final Smash hit cancel",
        "disable final start camera" => "no Final Smash start camera",
        "add partial mtion sword when changeing" => "sword animation carries over",
        "request remove have craft weapon" => "put the tool away",
        "disable furafura voice final" => "no dizzy voice",
        "disable capture pulled se" => "no pulled-in sound",
        "input check" => "reading the buttons",
        "request squat se" => "crouch sound",
        "no speed operation chk" => "keep speed as it is",
        "slide enable control" => "can steer while sliding",
        "slide enable gravity" => "falls while sliding",
        "sttus end if pickup failed" => "end if the pickup fails",
        "hoist start" => "lift starts",
        "region jp" => "Japanese version",
        "hit cancel" => "cancel on hit",
        "motion change enable" => "animation can change",
        "move trans" => "moves with the animation",
        "enable control energy" => "player can steer",
        "reserve gravity stable unable" => "gravity off",
        "is check dive" => "dive check",
        other => return other.to_owned(),
    }
    .to_owned()
}

/// One argument as words: a game name loses its prefix, a text name its quotes.
fn arg_words(arg: &str) -> String {
    let arg = arg.trim();
    if arg.starts_with("Hash40::new_raw(") || arg.starts_with("0x") {
        return "(unnamed)".into();
    }
    if let Some(name) = arg.strip_prefix("Hash40::new(") {
        return name
            .trim_matches(|c| c == '"' || c == ')')
            .replace('_', " ");
    }
    if arg.starts_with('*')
        || arg
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
            && arg.contains('_')
    {
        return move_words(&symbol_words(arg));
    }
    arg.to_owned()
}

/// What a game module is about, for calls that have no better name.
fn module_words(module: &str) -> &str {
    match module {
        "AttackModule" => "Hitbox",
        "GrabModule" => "Grab",
        "WorkModule" => "Game value",
        "ItemModule" => "Held item",
        "MotionModule" => "Animation",
        "KineticModule" => "Movement",
        "EffectModule" => "Effect",
        "SoundModule" => "Sound",
        "ControlModule" => "Controller",
        "CameraModule" => "Camera",
        "ArticleModule" => "Object",
        "VisibilityModule" | "ModelModule" => "Model",
        "GroundModule" => "Ground",
        "PostureModule" => "Position",
        "StatusModule" => "Action",
        "DamageModule" => "Damage",
        "HitModule" => "Hurtbox",
        "ShieldModule" | "ReflectorModule" => "Shield",
        "CatchModule" => "Grab",
        "SlowModule" => "Slowdown",
        "SearchModule" => "Detection",
        "AreaModule" => "Area",
        "JostleModule" => "Push box",
        "ColorBlendModule" => "Fighter tint",
        "ShakeModule" => "Shake",
        "LinkModule" => "Link",
        _ => "Game",
    }
}

/// "LandingHeavyRumble" becomes "landing heavy rumble".
fn camel_words(name: &str) -> String {
    let mut out = String::new();
    for (index, character) in name.chars().enumerate() {
        if character.is_uppercase() && index > 0 {
            out.push(' ');
        }
        out.extend(character.to_lowercase());
    }
    out
}

/// A game call as a short phrase: `WorkModule::on_flag(.., *X_FLAG_Y)` becomes `Turn on "y"`.
pub fn call_words(callee: &str, args: &[String]) -> String {
    let callee = callee
        .trim()
        .trim_start_matches("macros::")
        .trim_end_matches('!');
    let args: Vec<&String> = args
        .iter()
        .filter(|arg| {
            !arg.starts_with("agent") && !arg.starts_with("fighter.") && arg.as_str() != "boma"
        })
        .collect();
    let (module, function) = callee.rsplit_once("::").unwrap_or(("", callee));
    let module = module.rsplit("::").next().unwrap_or(module);
    let first = || args.first().map(|arg| arg_words(arg)).unwrap_or_default();
    let second = || args.get(1).map(|arg| arg_words(arg)).unwrap_or_default();
    if let Some(common) = function
        .split_once("_game_")
        .or_else(|| function.split_once("_expression_"))
        .or_else(|| function.split_once("_effect_"))
        .or_else(|| function.split_once("_sound_"))
    {
        return format!(
            "Use the shared \"{}\" part",
            camel_words(common.1.trim_end_matches("Common"))
        );
    }
    match function {
        "on_flag" => format!("Turn on \"{}\"", first()),
        "off_flag" => format!("Turn off \"{}\"", first()),
        "set_int" | "set_float" | "set_int64" => format!("Set \"{}\" to {}", second(), first()),
        "inc_int" => format!("Count up \"{}\"", first()),
        "dec_int" => format!("Count down \"{}\"", first()),
        "add_int" | "add_float" => format!("Add {} to \"{}\"", first(), second()),
        "notify_event_msc_cmd" => notice_words(&args),
        "set_rumble" | "RUMBLE_HIT" | "rumble" => "Rumble".into(),
        "PLAY_SE" | "PLAY_SE_REMAIN" | "PLAY_SE_NO_3D" => format!("Play sound {}", first()),
        "PLAY_STEP" | "PLAY_STEP_FLIPPABLE" | "PLAY_LANDING_SE" => "Footstep sound".into(),
        "PLAY_SEQUENCE" | "PLAY_FLY_VOICE" | "PLAY_VC" => "Voice line".into(),
        "PLAY_STATUS" => format!("Play sound {} for this action", first()),
        "SET_PLAY_INHIVIT" => format!("Don't repeat {} for a while", first()),
        "STOP_SE" => format!("Stop sound {}", first()),
        "set_devil" => "Devil form".into(),
        "set_have_item_action" => "Held item animation".into(),
        "set_have_item_scale_anim" => "Held item size animation".into(),
        "pickup_item" => "Pick up an item".into(),
        "use_item" => "Use the held item".into(),
        "set_attach_item_visibility" => "Show or hide the attached item".into(),
        "set_have_item_visibility" => "Show or hide the held item".into(),
        "set_rebound" if module == "GrabModule" => "Grab bounce-off".into(),
        "change_kinetic" => format!(
            "Movement type: {}",
            first().replace("fighter kinetic type ", "")
        ),
        "clear" if module == "AttackModule" => "End one hitbox".into(),
        "set_power_mul_status" | "set_power_mul" => {
            format!("Damage multiplier: {}", first())
        }
        "set_visibility" | "set_whole" => "Show or hide the model".into(),
        "clear_all" if module == "AttackModule" => "End all hitboxes".into(),
        "wait_loop_sync_mot" => "Match the animation loop".into(),
        "GET_VOICE_REGION" | "GET_VOICE_VARIATION" => "Pick a voice".into(),
        "request_paint" => "Paint the fighter".into(),
        "scale" => "Fighter size".into(),
        "get_sum_speed_y" => "falling speed".into(),
        "get_float" | "get_int" => format!("\"{}\"", first()),
        "is_flag" => format!("\"{}\" is on", first()),
        "ATTACK" => "Hitbox".into(),
        "FT_IS_SAME_FIGHTER_CATEGORY" => "Check the fighter kind".into(),
        "battle_object" => "Read the fighter".into(),
        "SA_SET" if first().contains("air") => "Put the fighter in the air".into(),
        "SA_SET" => "Put the fighter on the ground".into(),
        _ => {
            let words = function
                .trim_start_matches("set_")
                .trim_start_matches("req_")
                .to_lowercase()
                .replace('_', " ");
            let words = move_words(&words);
            if module.is_empty() {
                let mut letters = words.chars();
                letters.next().map_or_else(String::new, |first| {
                    first.to_uppercase().chain(letters).collect()
                })
            } else {
                format!("{}: {words}", module_words(module))
            }
        }
    }
}

/// The game's own notices, named by what they change.
fn notice_words(args: &[&String]) -> String {
    let event = args.first().map_or("", |arg| arg.as_str());
    let value = args.get(1).map_or("", |arg| arg.as_str());
    match event {
        "Hash40::new_raw(0x2127e37c07)" => {
            match value.trim_start_matches("*GROUND_CLIFF_CHECK_KIND_") {
                "ALWAYS_BOTH_SIDES" => "Can grab the ledge from either side".into(),
                "ALWAYS" => "Can grab the ledge".into(),
                "NONE" => "Can't grab the ledge".into(),
                _ => "Change ledge grabbing".into(),
            }
        }
        "Hash40::new_raw(0x23c33f3bdc)" => "Face the other way".into(),
        "Hash40::new_raw(0x26769bd1de)" => "Tilt the body".into(),
        "Hash40::new_raw(0x2ea0f68425)" => format!(
            "Pass through other fighters {}",
            if value == "true" { "on" } else { "off" }
        ),
        "Hash40::new_raw(0x25fd66ecef)" => "Zoom the camera".into(),
        "Hash40::new_raw(0x2d51fcdb09)" => "Keep firing while the button is held".into(),
        _ => "Signal the fighter's own code".into(),
    }
}

/// A condition from the game's code as words that read after "If".
pub fn condition_words(code: &str) -> String {
    let code = code.trim().trim_end_matches('{').trim();
    let code = code
        .strip_prefix('(')
        .and_then(|inner| inner.strip_suffix(')'))
        .filter(|inner| split_args(inner).len() == 1)
        .unwrap_or(code)
        .trim();
    if code == "false" {
        return "never (the game switched this off)".into();
    }
    if code == "true" {
        return "always".into();
    }
    // The script dump writes some reads of the fighter's numbers as a bare address. These
    // compare a number with a value.
    if let Some((address, rest)) = code.split_once('(') {
        if address.starts_with("0x") {
            let args = split_args(rest.strip_suffix(')').unwrap_or(rest));
            if let [symbol, value] = args.as_slice() {
                if symbol.contains("WORK_ID_INT_COLOR") || symbol.contains("WORK_INT_COLOR") {
                    if let Ok(slot) = value.trim().parse::<i64>() {
                        return costume_words(NativeComparison::Eq, slot);
                    }
                }
                if symbol.contains("_INT_") {
                    return format!(
                        "{} is {}",
                        move_words(&symbol_words(symbol)),
                        value_words(value)
                    );
                }
            }
        }
    }
    if code.contains("L2CValue") && code.contains("operator<=") && code.contains("-0.4") {
        return "falling fast".into();
    }
    if code.contains("L2CValue") && code.contains("VOICE_DEFAULT") {
        return "the game uses its usual voices".into();
    }
    if code.contains("L2CValue") || code.starts_with("0x") {
        return "a check inside the game passes".into();
    }
    for (joiner, word) in [("&&", " and "), ("||", " or ")] {
        if code.contains(joiner) {
            return code
                .split(joiner)
                .map(condition_words)
                .collect::<Vec<_>>()
                .join(word);
        }
    }
    if let Some(inner) = code.strip_prefix('!') {
        let words = condition_words(inner);
        return match words.strip_suffix(" is on") {
            Some(name) => format!("{name} is off"),
            None => format!("not {words}"),
        };
    }
    for (operator, word) in [
        ("==", "is"),
        ("!=", "is not"),
        (">=", "is at least"),
        ("<=", "is at most"),
        (">", "is more than"),
        ("<", "is less than"),
    ] {
        if let Some((left, right)) = code.split_once(operator) {
            if left.contains("speed_y") {
                return format!("the up-down speed {word} {}", arg_words(right));
            }
            return format!(
                "{} {word} {}",
                condition_words(left).trim_end_matches(" is on"),
                arg_words(right)
            );
        }
    }
    match code.split_once('(') {
        Some((callee, rest)) => {
            let callee = callee.trim().trim_start_matches("macros::");
            let args = split_args(rest.strip_suffix(')').unwrap_or(rest));
            match callee {
                "is_excute" => "the move is running".into(),
                "IS_RANDOM" => "a random roll passes".into(),
                "FT_IS_SAME_FIGHTER_CATEGORY" => "this is a matching kind of fighter".into(),
                "IS_HAVE_ITEM_KIND" => "holding that item".into(),
                "is_exist_article" | "ArticleModule::is_exist" => "the object is out".into(),
                "IS_FIGHTER_STATUS_KIND" | "IS_STATUS_KIND" => match args.last() {
                    Some(status) => format!(
                        "the fighter is doing {}",
                        crate::gimmicks::field_canvas::status_name(status.trim_start_matches('*'))
                            .to_lowercase()
                    ),
                    None => "the fighter is in that action".into(),
                },
                _ => call_words(callee, &args),
            }
        }
        None => arg_words(code),
    }
}

/// One line of game code as words.
pub fn code_words(line: &str) -> String {
    let line = line.trim().trim_end_matches(';').trim();
    if let Some(condition) = line.strip_prefix("if ").or_else(|| {
        line.strip_prefix("if(")
            .map(|rest| rest.trim_end_matches('{').trim_end().trim_end_matches(')'))
    }) {
        return format!("If {}", condition_words(condition));
    }
    if line.starts_with("else") || line.starts_with("} else") {
        return "Otherwise".into();
    }
    if let Some((_, value)) = line
        .strip_prefix("let ")
        .and_then(|rest| rest.split_once('='))
    {
        return format!("Read {}", code_words(value).to_lowercase());
    }
    if line.starts_with("loop") || line.starts_with("for ") || line.starts_with("while ") {
        return "Repeat".into();
    }
    if line.starts_with("frame(") || line.starts_with("wait(") {
        return "Wait".into();
    }
    let words = match line.split_once('(') {
        Some((callee, _)) if callee.starts_with("0x") || callee.contains("L2CValue") => {
            "Leftover game code".into()
        }
        Some((callee, rest)) => {
            let rest = rest.trim_end_matches('{').trim_end();
            call_words(callee, &split_args(rest.strip_suffix(')').unwrap_or(rest)))
        }
        None => "Game code".into(),
    };
    if words.starts_with('"') {
        format!("Check whether {words}")
    } else {
        words
    }
}

/// Where a box sits, by bone: "top" is the fighter's centre.
pub fn bone_words(bone: &str) -> String {
    match bone {
        "top" | "rot" | "trans" => "body".into(),
        other => other.replace('_', " ").to_lowercase(),
    }
}

/// An edit error in words a player uses. The technical text stays under Details.
pub fn error_words(error: &str) -> String {
    let line = error.lines().next().unwrap_or(error);
    // "tracks[0].nodes[3]#12: reason" names the place in the saved file; the reason is enough.
    let line = match line.rsplit_once("]: ").or_else(|| line.rsplit_once(": ")) {
        Some((place, reason)) if place.contains("tracks[") || place.contains("nodes[") => reason,
        _ => line,
    };
    let mut text = format!(" {line} ");
    for (from, to) in [
        ("execute block", "action block"),
        (" Execute ", " action "),
        (" execute ", " action "),
        ("native ", "game "),
        ("Native ", "Game "),
        ("Game track", "Action lane"),
        ("Effect track", "Visuals lane"),
        ("Sound track", "Sound lane"),
        ("Expression track", "Rumble & camera lane"),
        (" root ", " top-level "),
        ("baseline", "game's original"),
        ("source export", "exported mod"),
        ("article", "object"),
        (" ACMD ", " move code "),
        ("Branch", "If"),
        (" motion ", " animation "),
    ] {
        text = text.replace(from, to);
    }
    let text = text.trim();
    let mut letters = text.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}

/// A game call's arguments for a card: the named ones (objects, animations, graphics). Plain
/// flags and numbers mean little without the code, which Show code gives.
pub fn call_detail(arguments: &[String]) -> String {
    arguments
        .iter()
        .filter(|arg| arg.starts_with('*') || arg.contains("Hash40::new(\""))
        .map(|arg| arg_words(arg))
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod common_sense_tests {
    use super::*;

    #[test]
    fn the_smash_release_check_is_a_when_and_game_values_read_as_words() {
        let released = MoveCondition::WorkFlag {
            symbol: "FIGHTER_STATUS_ATTACK_FLAG_SMASH_SMASH_HOLD_TO_ATTACK".into(),
            expected: true,
        };
        assert_eq!(
            release_point(&released),
            Some("the smash attack is let go (charged or not)")
        );
        assert_eq!(
            condition_words("0x1050b0(*FIGHTER_INSTANCE_WORK_ID_INT_COLOR, 3)"),
            "the fighter wears costume 4 (c03)"
        );
        assert_eq!(
            condition_words(
                "0x370870(*FIGHTER_PICKEL_INSTANCE_WORK_ID_INT_HAVE_CRAFT_WEAPON_MATERIAL_KIND, *FIGHTER_PICKEL_MATERIAL_KIND_GOLD)"
            ),
            "held tool material is gold"
        );
        assert_eq!(condition_words("false"), "never (the game switched this off)");
        assert_eq!(negate("piece is out"), "piece is not out");
    }
}
