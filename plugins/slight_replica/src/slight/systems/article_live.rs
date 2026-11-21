use std::collections::{HashMap, HashSet};

/// Identity of a live article owned by the currently selected fighter.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ArticleKey {
    pub owner_kind: i32,
    pub article_kind: i32,
    pub agent: String,
}

/// The fighter currently selected in Visionary.
///
/// `costumes` is empty for an ordinary fighter (all slots match) and non-empty
/// for a slot-scoped character such as Shy Guy on Pac-Man c40–c47 so other
/// slots of the same base fighter are unaffected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedFighter {
    pub kind: i32,
    pub costumes: Vec<i32>,
}

/// Snapshot of one live battle object used by pure owner-chain resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectSnapshot {
    pub boid: u32,
    /// `0` = fighter (`BATTLE_OBJECT_CATEGORY_FIGHTER`), `1` = weapon (`BATTLE_OBJECT_CATEGORY_WEAPON`).
    pub category: i32,
    pub kind: i32,
    /// Fighter costume index (`FIGHTER_INSTANCE_WORK_ID_INT_COLOR`), or `-1` when unknown / non-fighter.
    pub costume: i32,
    /// Immediate owner battle-object id (fighter or parent weapon), if known.
    pub parent_boid: Option<u32>,
}

const MAX_OWNER_HOPS: usize = 8;

/// Resolve a weapon battle object to its [`ArticleKey`] if and only if its
/// owning fighter (including nested weapon owners) matches `selected` and its
/// article kind maps to a known agent name.
pub fn resolve_selected_owner_article<F, A>(
    boid: u32,
    selected: Option<&SelectedFighter>,
    lookup: F,
    agent_for_kind: A,
) -> Option<ArticleKey>
where
    F: Fn(u32) -> Option<ObjectSnapshot>,
    A: Fn(i32) -> Option<String>,
{
    let selected = selected?;
    let weapon = lookup(boid)?;
    if weapon.category != 1 {
        return None;
    }

    let mut visited = HashSet::with_capacity(MAX_OWNER_HOPS);
    visited.insert(boid);
    let mut current = weapon.parent_boid?;
    let mut owner_fighter = None;

    for _ in 0..MAX_OWNER_HOPS {
        if !visited.insert(current) {
            return None;
        }
        let candidate = lookup(current)?;
        if candidate.category == 0 {
            owner_fighter = Some(candidate);
            break;
        }
        if candidate.category == 1 {
            current = candidate.parent_boid?;
        } else {
            return None;
        }
    }

    let owner = owner_fighter?;
    if owner.kind != selected.kind {
        return None;
    }
    if !selected.costumes.is_empty() && !selected.costumes.contains(&owner.costume) {
        return None;
    }

    let agent = agent_for_kind(weapon.kind)?;
    Some(ArticleKey {
        owner_kind: owner.kind,
        article_kind: weapon.kind,
        agent,
    })
}

/// Separate fighter vs article capture tables keyed by `(fighter_kind, motion)`
/// and `(agent, motion)` respectively. Because `fighter_kind` and `article_kind`
/// are category-local and collide numerically (e.g. `0` is both Mario and
/// Mario's fireball / another fighter's weapon 0), keeping the tables distinct
/// prevents article captures from ever entering the fighter table.
#[derive(Default, Debug)]
pub struct CaptureRoutingTables {
    fighter: HashMap<(i32, u64), Vec<String>>,
    article: HashMap<(String, u64), Vec<String>>,
}

impl CaptureRoutingTables {
    pub fn record_fighter(&mut self, fighter_kind: i32, motion: u64, line: String) {
        self.fighter
            .entry((fighter_kind, motion))
            .or_default()
            .push(line);
    }

    pub fn record_article(&mut self, key: &ArticleKey, motion: u64, line: String) {
        self.article
            .entry((key.agent.clone(), motion))
            .or_default()
            .push(line);
    }

    pub fn fighter_lines(&self, fighter_kind: i32, motion: u64) -> &[String] {
        self.fighter
            .get(&(fighter_kind, motion))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn article_lines(&self, agent: &str, motion: u64) -> &[String] {
        self.article
            .get(&(agent.to_string(), motion))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}
