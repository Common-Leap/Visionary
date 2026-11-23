//! Which costumes of its fighter a live rule or a meter runs on.
//!
//! A character made in the editor shares its donor's fighter kind and owns some of its costumes,
//! so a rule names costumes as well as a fighter. Plain std only: the editor's tests include
//! this file by path and run it, since tests in the plugin crate never run.

/// The costume half of `runs_on`: `costumes` lists the only costumes a rule runs on (empty =
/// all), `except_costumes` the ones it never runs on. An unknown costume (-1) passes.
pub fn costume_is(color: i32, costumes: &[u8], except_costumes: &[u8]) -> bool {
    let Ok(color) = u8::try_from(color) else {
        return true;
    };
    (costumes.is_empty() || costumes.contains(&color)) && !except_costumes.contains(&color)
}

/// The costume a rule is tested against. A fighter's own (`acting_color`); for a projectile or
/// other object, the costume of the fighter that made it (`founder`: its kind and costume) when
/// the rule is for that fighter, so Mario's version of a move and a made character's each drive
/// only their own fireballs. Anything else stays unknown (-1) and passes, as before.
pub fn rule_color(acting_color: i32, rule_kind: Option<i32>, founder: Option<(i32, i32)>) -> i32 {
    if acting_color >= 0 {
        return acting_color;
    }
    match (rule_kind, founder) {
        (Some(kind), Some((founder_kind, color))) if kind == founder_kind => color,
        _ => -1,
    }
}

/// Who is making the call a rule is tested against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Acting {
    /// The fighter's kind; -1 for anything that isn't a fighter.
    pub fighter: i32,
    /// That fighter's costume; -1 for anything that isn't a fighter.
    pub color: i32,
    /// The kind of a projectile or other object a fighter made; -1 for anything else. Fighters
    /// and objects number their kinds separately, so the two are never compared.
    pub weapon: i32,
    /// For an object: the kind and costume of the fighter that made it, when it is known.
    pub founder: Option<(i32, i32)>,
}

/// What a rule was made for.
#[derive(Clone, Copy, Debug)]
pub struct RuleScope<'a> {
    pub fighter: Option<i32>,
    /// The object the rule is for; `None` for a rule of the fighter's own move.
    pub weapon: Option<i32>,
    pub costumes: &'a [u8],
    pub except_costumes: &'a [u8],
}

/// Whether a rule runs on what is acting. A fighter's rule runs only on fighters and an object's
/// rule only on that object, since the two share motion names. `plays_as(wanted, kind)` says
/// whether a fighter of `kind` runs rules made for `wanted`.
pub fn runs_on(acting: Acting, rule: RuleScope<'_>, plays_as: impl Fn(i32, i32) -> bool) -> bool {
    let fighter_is = |kind: i32| {
        rule.fighter
            .is_none_or(|wanted| kind == -1 || plays_as(wanted, kind))
    };
    let object = match rule.weapon {
        Some(weapon) => {
            acting.weapon == weapon && fighter_is(acting.founder.map_or(-1, |(kind, _)| kind))
        }
        None => acting.weapon == -1 && fighter_is(acting.fighter),
    };
    // Whoever runs the rule's fighter's rules wears its costumes.
    let founder = acting.founder.map(|(kind, color)| match rule.fighter {
        Some(wanted) if plays_as(wanted, kind) => (wanted, color),
        _ => (kind, color),
    });
    object
        && costume_is(
            rule_color(acting.color, rule.fighter, founder),
            rule.costumes,
            rule.except_costumes,
        )
}

/// The fighter, object and motion a rule is for; rules with equal scopes are versions of one
/// move.
pub type MoveScope = (Option<i32>, Option<i32>, Option<u64>);

/// A live rule that can be limited to some costumes of its fighter.
pub trait CostumeScoped {
    fn scope(&self) -> MoveScope;
    /// For a rule of an object's script: the object's agent name ("mario_fireball") it came with,
    /// the kind that names in the running game, and the fighter the rule is for.
    fn object(&mut self) -> Option<(&str, &mut Option<i32>, &mut Option<i32>)>;
    fn costumes(&self) -> &[u8];
    fn except_costumes_mut(&mut self) -> &mut Vec<u8>;
}

/// The most specific rule wins: when a rule of a move names costumes, every rule of the same
/// fighter and move that names none stops running on them. So Mario's version of a jab and a
/// made character's version for c08-c15 can both be live, each on its own costumes. Done once
/// when a rule list arrives, so matching stays a slice scan.
pub fn prefer_specific_costumes<R: CostumeScoped>(rules: &mut [R]) {
    let claims: Vec<(MoveScope, Vec<u8>)> = rules
        .iter()
        .filter(|rule| !rule.costumes().is_empty() && rule.scope().0.is_some())
        .map(|rule| (rule.scope(), rule.costumes().to_vec()))
        .collect();
    if claims.is_empty() {
        return;
    }
    for rule in rules.iter_mut() {
        if !rule.costumes().is_empty() || rule.scope().0.is_none() {
            continue;
        }
        let scope = rule.scope();
        let except = rule.except_costumes_mut();
        for slot in claims
            .iter()
            .filter(|(claim, _)| *claim == scope)
            .flat_map(|(_, slots)| slots)
        {
            if !except.contains(slot) {
                except.push(*slot);
            }
        }
    }
}

/// Give every object rule the kind its object has in the running game. `object_of` answers an
/// agent name with that kind, and with the fighter that has to have made the object where the
/// kind alone does not say. A rule whose object the game does not have is dropped; those
/// objects' names are returned, to be refused by name.
pub fn name_objects<R: CostumeScoped>(
    rules: &mut Vec<R>,
    object_of: impl Fn(&str) -> Option<(i32, Option<i32>)>,
) -> Vec<String> {
    let mut unknown = Vec::new();
    rules.retain_mut(|rule| {
        let Some((agent, kind, fighter)) = rule.object() else {
            return true;
        };
        let Some((weapon, maker)) = object_of(agent) else {
            if !unknown.iter().any(|name| name == agent) {
                unknown.push(agent.to_owned());
            }
            return false;
        };
        *kind = Some(weapon);
        *fighter = maker.or(*fighter);
        true
    });
    unknown
}
