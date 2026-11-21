//! Which costumes of its fighter a live rule or a meter runs on.
//!
//! A character made in the editor shares its donor's fighter kind and owns some of its costumes,
//! so a rule names costumes as well as a fighter. Plain std only: the editor's tests include
//! this file by path and run it, since tests in the plugin crate never run.

/// The costume half of `acting_is`: `costumes` lists the only costumes a rule runs on (empty =
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

/// The fighter kind and motion a rule is for; rules with equal scopes are versions of one move.
pub type MoveScope = (Option<i32>, Option<u64>);

/// A live rule that can be limited to some costumes of its fighter.
pub trait CostumeScoped {
    fn scope(&self) -> MoveScope;
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
