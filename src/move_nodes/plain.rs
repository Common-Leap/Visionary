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
        "Notify event" => "Game notice",
        "Change status" => "Go to another action",
        "Allow transition" | "Allow transition group" => "Allow cancel",
        "Disable transition" | "Disable transition group" => "Block cancel",
        "Generate article" => "Spawn object",
        "Generate enabled article" => "Spawn object if allowed",
        "Article state" => "Object state",
        "Remove article" => "Remove object",
        "Article visibility" => "Show or hide object",
        "Launch article" => "Launch object",
        "Launch if present" => "Launch object if out",
        "Article motion" => "Object animation",
        "Send event" => "Tell a gimmick",
        "Effect spawn" | "Spawn effect" => "Visual effect",
        "Slope" | "Slope alignment" => "Lean with the ground",
        "Effect off" | "Native effect off" => "End visual effect",
        "Rumble pattern" => "Rumble",
        "Hit rumble" => "Rumble on hit",
        "Default catch setup" => "Standard grab setup",
        "Body collision" | "Body collision area" | "Body collision area XY" => "Push box",
        "Imported actions" => "Actions",
        "Imported effects" => "Visual effects",
        "Top-level command" | "Top-level sound" => "Action",
        "Native structure" => "Original game code",
        "Native setter" | "Native call" | "Native slowdown" => "Game code",
        "New controller" => "New gimmick",
        "Controller" => "Gimmick",
        "Event source" => "When",
        "Entry state" => "First state",
        "Attack mod" => "Change hitbox power",
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
        "If" => "Checks something during the move. Yes runs when it is true, No when it is not, and After runs either way.",
        "Repeat" => "Runs the blocks in Loop several times.",
        "Hitbox" => "An area that hits opponents. It stays on until an End hitbox block.",
        "End all hitboxes" => "Stops every hitbox this move has turned on.",
        "End one hitbox" => "Stops one hitbox and leaves the others on.",
        "Grab box" => "An area that grabs opponents instead of hitting them.",
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
fn symbol_words(symbol: &str) -> String {
    let symbol = symbol.trim_start_matches('*');
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
            "the smash attack was charged",
            "the smash attack was not charged",
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
            symbol_words(symbol),
            if expected { "on" } else { "off" }
        ),
    }
}

/// A condition as words that read after "If".
pub fn condition_sentence(condition: &MoveCondition) -> String {
    match condition {
        MoveCondition::IsExecute => "the move is running".into(),
        MoveCondition::Source { .. } => "the game's own check passes".into(),
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
            format!("\"{}\" {} {right}", symbol_words(symbol), compare(*op))
        }
        MoveCondition::WorkFloat { symbol, op, value } => {
            format!("\"{}\" {} {value}", symbol_words(symbol), compare(*op))
        }
        MoveCondition::And { .. } => "all of these are true".into(),
        MoveCondition::Or { .. } => "any of these is true".into(),
        MoveCondition::Not(inner) => format!("not: {}", condition_sentence(inner)),
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
