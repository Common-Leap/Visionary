//! Status entry points share controller transitions and their evaluation limits.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const CANCEL_FEATURE: &str = "status_cancel.v1";
pub const FEATURE: &str = "status_authoring.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Phase {
    Pre,
    Init,
    MainEnter,
    MainLoop,
    Exec,
    End,
    Exit,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Policy {
    #[default]
    Native,
    Owned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusBinding {
    pub target: StatusRef,
    #[serde(default)]
    pub policy: Policy,
    #[serde(default)]
    pub roots: BTreeMap<Phase, Vec<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateStatus {
    pub state: String,
    pub binding: StatusBinding,
}

/// Checks shared by source and wire models. Resolve aliases before comparing owners.
pub fn validate<'a>(
    fighter: Option<&str>,
    initial: &str,
    bindings: impl IntoIterator<Item = (&'a str, &'a StatusBinding)>,
    transitions: &[(&str, &str)],
    ordinary: &BTreeSet<usize>,
) -> Result<(), String> {
    let mut owners = BTreeSet::new();
    let mut roots = BTreeSet::new();
    let mut states = BTreeSet::new();
    for (state, binding) in bindings {
        if !states.insert(state) {
            return Err(format!("duplicate status binding for {state:?}"));
        }
        if state == initial {
            return Err("initial controller state must be unbound".into());
        }
        binding.target.validate_key()?;
        if matches!(binding.target, StatusRef::Custom { .. }) && binding.policy != Policy::Owned {
            return Err("custom status requires Owned policy".into());
        }
        if let Some(fighter) = fighter {
            let id = binding.target.resolve(fighter)?;
            if !owners.insert(id) {
                return Err(format!("duplicate resolved status owner {fighter}/{id}"));
            }
        }
        for (&phase, indices) in &binding.roots {
            if indices.len() > super::live_model::MAX_TRANSITIONS {
                return Err("status phase root exceeds transition limit".into());
            }
            for &index in indices {
                let Some(&(from, to)) = transitions.get(index) else {
                    return Err(format!("status phase root index {index} is out of range"));
                };
                if from != state || (phase != Phase::MainLoop && to != state) {
                    return Err(format!(
                        "status {phase:?} root must be an owner-state self edge"
                    ));
                }
                if !roots.insert(index) || ordinary.contains(&index) {
                    return Err(format!("status root {index} has another dispatcher owner"));
                }
            }
        }
        if binding.policy == Policy::Owned
            && [Phase::Pre, Phase::MainEnter]
                .iter()
                .any(|p| binding.roots.get(p).is_none_or(Vec::is_empty))
        {
            return Err("Owned status requires Pre and MainEnter roots".into());
        }
    }
    Ok(())
}

pub fn phase<'a>(
    bindings: impl IntoIterator<Item = &'a StatusBinding>,
    index: usize,
) -> Option<Phase> {
    bindings
        .into_iter()
        .flat_map(|b| &b.roots)
        .find_map(|(&phase, roots)| roots.contains(&index).then_some(phase))
}

/// Stable installation identity. Body edits do not alter the installed callbacks.
pub fn manifest(
    programs: impl IntoIterator<Item = super::live_model::LiveProgram>,
) -> Result<BTreeSet<(String, u32, u8)>, String> {
    let mut manifest = BTreeSet::new();
    let mut owners = BTreeSet::new();
    let mut allocations: BTreeMap<(String, String), u8> = BTreeMap::new();
    let mut slots: BTreeMap<(String, u8), String> = BTreeMap::new();
    for program in programs {
        validate_allocations(&program.fighter, &program.custom_statuses)?;
        for (key, slot) in &program.custom_statuses {
            if allocations
                .insert((program.fighter.clone(), key.clone()), *slot)
                .is_some_and(|old| old != *slot)
                || slots
                    .insert((program.fighter.clone(), *slot), key.clone())
                    .is_some_and(|old| old != *key)
            {
                return Err("conflicting custom status manifests".into());
            }
            // Install tombstones too: smashline expands the table before construction.
            manifest.insert((
                program.fighter.clone(),
                native_count(&program.fighter)? + 1 + u32::from(*slot),
                Policy::Owned as u8,
            ));
        }
        for entry in program.status_bindings {
            entry.binding.target.in_manifest(&program.custom_statuses)?;
            let id = entry.binding.target.resolve(&program.fighter)?;
            if !owners.insert((program.fighter.clone(), id)) {
                return Err(format!(
                    "duplicate resolved status owner {}/{id}",
                    program.fighter
                ));
            }
            manifest.insert((program.fighter.clone(), id, entry.binding.policy as u8));
        }
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> StatusBinding {
        StatusBinding {
            target: "FIGHTER_STATUS_KIND_SPECIAL_S".into(),
            policy: Policy::Owned,
            roots: [(Phase::Pre, vec![0]), (Phase::MainEnter, vec![1])].into(),
        }
    }
    #[test]
    fn roots_are_bounded_exclusive_and_phase_scoped() {
        let transitions = [("cape", "cape"), ("cape", "cape")];
        let check = |b: &StatusBinding, initial, ordinary: &BTreeSet<usize>| {
            validate(
                Some("mario"),
                initial,
                [("cape", b)],
                &transitions,
                ordinary,
            )
        };
        let mut b = binding();
        assert!(check(&b, "idle", &BTreeSet::new()).is_ok());
        assert!(check(&b, "cape", &BTreeSet::new())
            .unwrap_err()
            .contains("initial"));
        assert!(check(&b, "idle", &[0].into())
            .unwrap_err()
            .contains("another dispatcher"));
        b.roots.insert(Phase::Exec, vec![2]);
        assert!(check(&b, "idle", &BTreeSet::new())
            .unwrap_err()
            .contains("out of range"));
        b.roots.remove(&Phase::Exec);
        b.target = "FIGHTER_CLOUD_STATUS_KIND_SPECIAL_LW_CHARGE".into();
        assert!(check(&b, "idle", &BTreeSet::new()).is_err());
        b.target = "FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL".into();
        assert!(check(&b, "idle", &BTreeSet::new()).is_err());
    }
    #[test]
    fn duplicate_native_id_and_non_loop_state_edges_are_rejected() {
        let b = binding();
        assert!(validate(
            Some("mario"),
            "idle",
            [("cape", &b), ("other", &b)],
            &[("cape", "cape"), ("cape", "cape")],
            &BTreeSet::new()
        )
        .unwrap_err()
        .contains("duplicate resolved"));
        assert!(validate(
            Some("mario"),
            "idle",
            [("cape", &b)],
            &[("cape", "idle"), ("cape", "cape")],
            &BTreeSet::new()
        )
        .unwrap_err()
        .contains("self edge"));
    }
}

/// Status phases deliberately admit only synchronous operations with a proven
/// delivery path, plus combat policies, which the owning Main line delivers.
/// Contact, article, item and other damage actions retain their hooks.
pub fn permits(phase: Phase, action: &super::live_model::Action) -> bool {
    use super::live_model::Action;
    match action {
        Action::Status(command) => command.permits(phase),
        Action::Set { .. }
        | Action::SetFlag { .. }
        | Action::Add { .. }
        | Action::Raise { .. }
        | Action::Clear { .. }
        | Action::Reset { .. }
        | Action::SetNative { .. }
        | Action::AddNative { .. }
        | Action::SetNativeFlag { .. } => true,
        Action::SetMotionRate { .. } | Action::AddSpeed { .. } => {
            !matches!(phase, Phase::Pre | Phase::End | Phase::Exit)
        }
        Action::RequestFighterStatus { .. } => phase == Phase::MainLoop,
        // Queued for the owning Main line under the status current at delivery;
        // Pre runs before that status exists.
        Action::CombatPolicy { .. } => phase != Phase::Pre,
        // Queued the same way, and each delivery re-checks the status and the capture link.
        Action::AddHeldOpponentDamage { .. }
        | Action::ThrowHeldOpponent { .. }
        | Action::ReleaseHeldOpponent => {
            matches!(phase, Phase::MainEnter | Phase::MainLoop | Phase::Exec)
        }
        _ => false,
    }
}

#[cfg(test)]
mod wire_tests {
    use super::super::live_model as m;
    use super::*;
    fn program() -> m::LiveProgram {
        serde_json::from_value(serde_json::json!({
            "version":m::MODEL_VERSION,"id":"status:mario","fighter":"mario",
            "initial":"idle","states":["idle","cape"],"vars":[],"transitions":[{
            "from":"idle","to":"idle","on_frame":true,"guard":{"Compare":{"op":"eq","left":{"Const":{"Int":1}},"right":{"Const":{"Int":1}}}},"actions":[]
            }],"bindings":[]
        }))
        .unwrap()
    }
    #[test]
    fn legacy_codec_and_feature_gate() {
        let mut p = program();
        p.version = 4;
        p.bindings.push(m::HookBinding {
            hook: m::HookName::OnStockStart,
            article: None,
            transitions: vec![0],
        });
        assert!(p.validate().is_ok());
        assert!(p.status_bindings.is_empty());
        p.status_bindings.push(StateStatus {
            state: "cape".into(),
            binding: StatusBinding {
                target: "FIGHTER_STATUS_KIND_SPECIAL_S".into(),
                policy: Policy::Native,
                roots: BTreeMap::new(),
            },
        });
        assert!(p.required_native_features().contains(FEATURE));
        assert!(p
            .validate()
            .unwrap_err()
            .iter()
            .any(|e| e.item == "version"));
        p.version = m::MODEL_VERSION;
        assert!(p.validate().is_ok());
        assert!(manifest([p.clone(), p.clone()])
            .unwrap_err()
            .contains("duplicate resolved"));
        let manifest = manifest([p.clone()]).unwrap();
        p.id = "body changed".into();
        assert_eq!(manifest, super::manifest([p.clone()]).unwrap());
        p.status_bindings[0].binding.policy = Policy::Owned;
        assert_ne!(manifest, super::manifest([p]).unwrap());
    }
    #[test]
    fn teardown_cannot_request_and_pre_cannot_sample_input() {
        let request = m::Action::RequestFighterStatus {
            symbol: "FIGHTER_STATUS_KIND_WAIT".into(),
        };
        assert!(permits(Phase::MainLoop, &request));
        assert!(!permits(Phase::End, &request));
        assert!(!permits(
            Phase::Pre,
            &m::Action::SetMotionRate {
                rate: m::Expr::Const(m::Value::Float(1.0))
            }
        ));
    }
}

/// Native symbols retain their legacy string encoding. Custom keys and slots
/// are package-local; every reference must match the persisted allocation map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum StatusRef {
    Native(String),
    Custom { key: String, slot: u8 },
}
impl From<String> for StatusRef {
    fn from(s: String) -> Self {
        Self::Native(s)
    }
}
impl From<&str> for StatusRef {
    fn from(s: &str) -> Self {
        Self::Native(s.into())
    }
}
impl std::ops::Deref for StatusRef {
    type Target = str;
    fn deref(&self) -> &str {
        match self {
            Self::Native(s) => s,
            Self::Custom { key, .. } => key,
        }
    }
}
impl StatusRef {
    pub fn validate_key(&self) -> Result<(), String> {
        if self.is_empty()
            || self.len() > super::live_model::MAX_SYMBOL_BYTES
            || self.contains('\0')
        {
            return Err("invalid status reference".into());
        }
        if matches!(self,Self::Custom {slot,..} if *slot>=16) {
            return Err("custom status slot exceeds 16/fighter".into());
        }
        Ok(())
    }
    pub fn resolve(&self, fighter: &str) -> Result<u32, String> {
        self.validate_key()?;
        match self {
            Self::Native(symbol) => super::registry::resolve(
                fighter,
                symbol,
                super::registry::IdentifierKind::FighterStatus,
            )
            .map(|e| e.value)
            .map_err(|e| e.to_string()),
            Self::Custom { slot, .. } => Ok(native_count(fighter)? + 1 + u32::from(*slot)),
        }
    }
    pub fn in_manifest(&self, ids: &BTreeMap<String, u8>) -> Result<(), String> {
        if let Self::Custom { key, slot } = self {
            if ids.get(key) != Some(slot) {
                return Err(
                    "custom status reference is absent from the allocation manifest".into(),
                );
            }
        }
        Ok(())
    }
}
pub fn validate_allocations(fighter: &str, ids: &BTreeMap<String, u8>) -> Result<(), String> {
    if ids.len() > 16 {
        return Err("custom status manifest exceeds 16/fighter including tombstones".into());
    }
    let mut slots = BTreeSet::new();
    for (key, slot) in ids {
        let target = StatusRef::Custom {
            key: key.clone(),
            slot: *slot,
        };
        target.resolve(fighter)?;
        if !slots.insert(slot) {
            return Err("duplicate custom status slot".into());
        }
    }
    Ok(())
}
/// Deleted keys remain allocated. New keys take the first unused slot.
pub fn allocate(ids: &mut BTreeMap<String, u8>, key: &str) -> Result<StatusRef, String> {
    let slot = match ids.get(key) {
        Some(slot) => *slot,
        None => (0..16)
            .find(|slot| !ids.values().any(|v| v == slot))
            .ok_or("custom status limit is 16/fighter, including tombstones")?,
    };
    let target = StatusRef::Custom {
        key: key.into(),
        slot,
    };
    target.validate_key()?;
    ids.insert(key.into(), slot);
    Ok(target)
}
// Pinned skyline-smash 258a566, lua_const.rs: FIGHTER_<OWNER>_STATUS_KIND_NUM.
// IDs are private to one package; bundles reject conflicting key/slot assignments.
pub fn native_count(fighter: &str) -> Result<u32, String> {
    match fighter {
        "bayonetta" => Ok(0x1F2),
        "brave" => Ok(0x1FE),
        "buddy" => Ok(0x1FB),
        "captain" => Ok(0x1EC),
        "cloud" => Ok(0x1F7),
        "dedede" => Ok(0x209),
        "demon" => Ok(0x213),
        "diddy" => Ok(0x1FF),
        "dolly" => Ok(0x203),
        "duckhunt" => Ok(0x1EB),
        "edge" => Ok(0x1F3),
        "eflame" => Ok(0x1EE),
        "element" => Ok(0x1E7),
        "elight" => Ok(0x1F1),
        "falco" => Ok(0x1E8),
        "fox" => Ok(0x1EA),
        "gamewatch" => Ok(0x205),
        "ganon" => Ok(0x1EC),
        "gaogaen" => Ok(0x1F4),
        "gekkouga" => Ok(0x1F1),
        "ike" => Ok(0x1F2),
        "inkling" => Ok(0x1F4),
        "jack" => Ok(0x1FC),
        "kamui" => Ok(0x1F4),
        "kirby" => Ok(0x37D),
        "koopajr" => Ok(0x1EF),
        "krool" => Ok(0x1F9),
        "link" => Ok(0x1E8),
        "littlemac" => Ok(0x1F4),
        "lucario" => Ok(0x1F0),
        "lucas" => Ok(0x1ED),
        "luigi" => Ok(0x1EE),
        "mario" => Ok(0x1E3),
        "mariod" => Ok(0x1E1),
        "marth" => Ok(0x1EB),
        "master" => Ok(0x1F8),
        "metaknight" => Ok(0x1F0),
        "mewtwo" => Ok(0x1E9),
        "miienemyf" => Ok(0x1FE),
        "miienemyg" => Ok(0x1F7),
        "miienemys" => Ok(0x1FA),
        "miifighter" => Ok(0x207),
        "miigunner" => Ok(0x1FA),
        "miiswordsman" => Ok(0x1F9),
        "murabito" => Ok(0x204),
        "ness" => Ok(0x1ED),
        "packun" => Ok(0x1F4),
        "pacman" => Ok(0x1F0),
        "palutena" => Ok(0x1E9),
        "pfushigisou" => Ok(0x1E5),
        "pickel" => Ok(0x21B),
        "pikachu" => Ok(0x1F0),
        "pikmin" => Ok(0x1E8),
        "plizardon" => Ok(0x1E6),
        "popo" => Ok(0x1EF),
        "purin" => Ok(0x1EA),
        "pzenigame" => Ok(0x1EA),
        "reflet" => Ok(0x1FD),
        "ridley" => Ok(0x202),
        "robot" => Ok(0x1EB),
        "rockman" => Ok(0x1F0),
        "rosetta" => Ok(0x1E6),
        "roy" => Ok(0x1F2),
        "ryu" => Ok(0x202),
        "samus" => Ok(0x1F6),
        "sheik" => Ok(0x1F3),
        "shizue" => Ok(0x210),
        "shulk" => Ok(0x1ED),
        "simon" => Ok(0x1EC),
        "snake" => Ok(0x201),
        "sonic" => Ok(0x1F3),
        "szerosuit" => Ok(0x1EC),
        "tantan" => Ok(0x1FF),
        "trail" => Ok(0x1F5),
        "wario" => Ok(0x200),
        "wiifit" => Ok(0x1EF),
        "wolf" => Ok(0x1EA),
        "yoshi" => Ok(0x1EF),
        "zelda" => Ok(0x1EB),
        _ => Err("fighter has no reviewed native status table bound".into()),
    }
}

#[cfg(test)]
mod allocation_tests {
    use super::*;
    #[test]
    fn custom_ids_survive_round_trip_removal_and_reinsertion() {
        let mut ids = BTreeMap::new();
        let first = allocate(&mut ids, "cape").unwrap();
        let second = allocate(&mut ids, "hold").unwrap();
        assert_eq!(first.resolve("mario").unwrap(), 0x1e4);
        assert_eq!(second.resolve("mario").unwrap(), 0x1e5);
        let raw = serde_json::to_string(&ids).unwrap();
        let mut ids = serde_json::from_str(&raw).unwrap();
        // Removing the owning state keeps its key as a tombstone.
        assert_eq!(allocate(&mut ids, "cape").unwrap(), first);
        assert_eq!(
            allocate(&mut ids, "new").unwrap().resolve("mario").unwrap(),
            0x1e6
        );
        for n in 3..16 {
            allocate(&mut ids, &format!("key{n}")).unwrap();
        }
        assert!(allocate(&mut ids, "overflow").is_err());
        assert!(first.in_manifest(&BTreeMap::new()).is_err());
        assert!(validate_allocations("unknown", &ids).is_err());
        assert!(validate_allocations("mario", &[("a".into(), 0), ("b".into(), 0)].into()).is_err());
    }
    #[test]
    fn allocation_identity_conflicts_and_tombstones_install() {
        let make = |key: &str, slot| -> super::super::live_model::LiveProgram {
            serde_json::from_value(serde_json::json!({"version":5,"id":"custom","fighter":"mario","initial":"idle","states":["idle"],"vars":[],"transitions":[],"bindings":[],"custom_statuses":{key:slot}})).unwrap()
        };
        let a = make("a", 0);
        assert_eq!(
            manifest([a.clone()]).unwrap(),
            [("mario".into(), 0x1e4, Policy::Owned as u8)].into()
        );
        assert!(manifest([a.clone(), make("b", 0)])
            .unwrap_err()
            .contains("conflicting"));
        assert!(manifest([a, make("a", 1)])
            .unwrap_err()
            .contains("conflicting"));
    }
}
