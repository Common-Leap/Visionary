//! Hook-to-entry-point registry (slice F2.2).
//!
//! A behavior transition needs to know *when* its guard is evaluated. That
//! "when" is a hook (`onDealDamage`, `onStockStart`, …), and a hook is only
//! real if a measurable engine point fires it. This registry is the closed
//! set of hook names, each mapped to its entry
//! point with evidence — or explicitly marked unavailable with the reason
//! and what would unblock it. There is no free-form hook string anywhere:
//! [`parse`] rejects unknown names, and [`resolve`] refuses unavailable
//! hooks, so the F2.3 emitter cannot emit a guessed hook.
//!
//! Entry points are plugin facts, not promises: each [`SupportState::Supported`]
//! cites the system and symbol that fires it.
//! Per-fighter *bindings* (which status, which article) are separate data
//! owned by later slices. A hook that cannot fire correctly without such a
//! binding stays unavailable here; support means the runtime path exists now,
//! not merely that a potentially useful feed exists.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The closed hook vocabulary. Serde spellings are the PLAN names
/// (`onDealDamage`, …), so manifests, projects, and diagnostics name one
/// hook the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::enum_variant_names)] // Mirrors the public onX hook vocabulary exactly.
pub enum HookName {
    OnDealDamage,
    OnTakeDamage,
    OnAttackShielded,
    OnShieldHit,
    OnIncomingHit,
    OnReflect,
    OnGrab,
    OnThrow,
    OnGrabbed,
    OnItemPickup,
    OnItemThrown,
    OnChargeTick,
    OnStore,
    OnSpend,
    OnStatusEnter,
    OnStatusExit,
    OnTimeout,
    OnWhiff,
    OnStockStart,
    /// PLAN spells the acronym `KO`, which camelCase would mangle.
    #[serde(rename = "onKO")]
    OnKo,
    OnArticleSpawn,
    OnArticleRemoved,
    OnArticleCatch,
    OnArticleTimeout,
    OnArticleTransfer,
    /// PLAN spells the acronym `RNG`, which camelCase would mangle.
    #[serde(rename = "onRNG")]
    OnRng,
    OnCounterHit,
    OnPin,
    OnCarry,
    OnMash,
    OnSwitch,
    OnAbsorb,
}

impl HookName {
    /// The PLAN spelling, used in diagnostics instead of a bare identifier.
    pub fn name(&self) -> &'static str {
        match self {
            HookName::OnDealDamage => "onDealDamage",
            HookName::OnTakeDamage => "onTakeDamage",
            HookName::OnAttackShielded => "onAttackShielded",
            HookName::OnShieldHit => "onShieldHit",
            HookName::OnIncomingHit => "onIncomingHit",
            HookName::OnReflect => "onReflect",
            HookName::OnGrab => "onGrab",
            HookName::OnThrow => "onThrow",
            HookName::OnGrabbed => "onGrabbed",
            HookName::OnItemPickup => "onItemPickup",
            HookName::OnItemThrown => "onItemThrown",
            HookName::OnChargeTick => "onChargeTick",
            HookName::OnStore => "onStore",
            HookName::OnSpend => "onSpend",
            HookName::OnStatusEnter => "onStatusEnter",
            HookName::OnStatusExit => "onStatusExit",
            HookName::OnTimeout => "onTimeout",
            HookName::OnWhiff => "onWhiff",
            HookName::OnStockStart => "onStockStart",
            HookName::OnKo => "onKO",
            HookName::OnArticleSpawn => "onArticleSpawn",
            HookName::OnArticleRemoved => "onArticleRemoved",
            HookName::OnArticleCatch => "onArticleCatch",
            HookName::OnArticleTimeout => "onArticleTimeout",
            HookName::OnArticleTransfer => "onArticleTransfer",
            HookName::OnRng => "onRNG",
            HookName::OnCounterHit => "onCounterHit",
            HookName::OnPin => "onPin",
            HookName::OnCarry => "onCarry",
            HookName::OnMash => "onMash",
            HookName::OnSwitch => "onSwitch",
            HookName::OnAbsorb => "onAbsorb",
        }
    }
}

/// Why [`parse`] failed: the name is outside the closed vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownHook {
    pub name: String,
}

impl fmt::Display for UnknownHook {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unknown hook {:?}; hooks are the closed PLAN set, not free-form strings",
            self.name
        )
    }
}

impl FromStr for HookName {
    type Err = UnknownHook;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        HOOKS
            .iter()
            .find(|def| def.hook.name() == raw)
            .map(|def| def.hook)
            .ok_or_else(|| UnknownHook {
                name: raw.to_string(),
            })
    }
}

/// A measurable engine point that fires hooks. Variants name plugin facts;
/// adding one requires the system and symbol that fires it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryPoint {
    /// Attacker-side collision records (`drain_collision_hits`).
    AttackerCollisionQueue,
    /// Defender-side collision records (`drain_defender_hits`).
    DefenderCollisionQueue,
    /// Per-frame processing (`timer_system::on_frame`).
    FrameTick,
    /// A scheduled countdown elapsing (`timer_system::schedule`).
    TimerExpiry,
    /// Fighter appearance (initial `Spawn` or identity-aware REBIRTH edge).
    FighterSpawn,
    /// Fighter loss (`GameEvent::Die` / `Kills` / `Suicide`).
    FighterDown,
    /// A status change carrying the new status id (`GameEvent::StatusChange`).
    StatusChange,
    /// A new weapon boid observed (`article_notifier`, `GameEvent::Spawn`).
    WeaponSpawn,
}

/// Whether a hook fires today. Supported cites the entry point and the
/// evidence; unavailable names the missing feed and what would unblock it.
/// Both are data, so review can see the whole board at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportState {
    Supported {
        entry: EntryPoint,
        evidence: &'static str,
    },
    /// The hook requires an exact reviewed status binding or one closed built-in resource source.
    StatusBound {
        entry: EntryPoint,
        evidence: &'static str,
    },
    Unavailable {
        reason: &'static str,
        unblocks: &'static str,
    },
}

/// One registry row: the hook plus its support state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookDef {
    pub hook: HookName,
    pub state: SupportState,
}

/// The full PLAN hook set, each with its verdict. This table is the slice:
/// every hook supported with evidence or explicitly unavailable.
pub static HOOKS: &[HookDef] = &[
    HookDef {
        hook: HookName::OnReflect,
        state: SupportState::Supported {
            entry: EntryPoint::AttackerCollisionQueue,
            evidence: "13.0.4 per-contact reflector result at 0x3df93c, weapon source and fighter receiver; sealed receiver delivery",
        },
    },
    HookDef {
        hook: HookName::OnItemPickup,
        state: SupportState::Supported {
            entry: EntryPoint::FrameTick,
            evidence: "item_control::observe: empty-to-held main-hand edge on owning Main; includes generated items",
        },
    },
    HookDef {
        hook: HookName::OnItemThrown,
        state: SupportState::Supported {
            entry: EntryPoint::FrameTick,
            evidence: "item_control::observe: held-to-empty main-hand edge on owning Main; includes drops and removal, not a throw-cause proof",
        },
    },
    HookDef {
        hook: HookName::OnGrab,
        state: SupportState::Supported {
            entry: EntryPoint::FrameTick,
            evidence: "grab_control::observe: rising reciprocal LINK_NO_CAPTURE child link on the owning fighter Main line",
        },
    },
    HookDef {
        hook: HookName::OnThrow,
        state: SupportState::Supported {
            entry: EntryPoint::FrameTick,
            evidence: "grab_control::observe: reciprocal child link released across consecutive THROW samples, with the released opponent identity still current",
        },
    },
    HookDef {
        hook: HookName::OnGrabbed,
        state: SupportState::Supported {
            entry: EntryPoint::FrameTick,
            evidence: "grab_control::observe: rising reciprocal LINK_NO_CAPTURE parent link on the captured fighter Main line",
        },
    },
    HookDef {
        hook: HookName::OnAttackShielded,
        state: SupportState::Supported {
            entry: EntryPoint::AttackerCollisionQueue,
            evidence: "HDR attack.rs x03df93c: per-contact AttackModule collision mask and defender ID; shield_contacts seals each shield contact with direct fighter or checked founder/current-owner credit",
        },
    },
    HookDef {
        hook: HookName::OnIncomingHit,
        state: SupportState::Supported {
            entry: EntryPoint::DefenderCollisionQueue,
            evidence: "signature-checked 13.0.4 DamageModule preparation at 0x3fe4b4; synchronous controller decision before percent and reaction",
        },
    },
    HookDef {
        hook: HookName::OnShieldHit,
        state: SupportState::Supported {
            entry: EntryPoint::DefenderCollisionQueue,
            evidence: "same native shield-result contact at 0x3df93c; exact defender stock and capture status, with saved get_power blocked attack power, never inferred percent or shield-HP depletion",
        },
    },
    HookDef {
        hook: HookName::OnDealDamage,
        state: SupportState::Supported {
            entry: EntryPoint::AttackerCollisionQueue,
            evidence: "damage_feed seals positive native percent commits and damage_dispatch routes exact fighter/weapon-owner credit after both stocks are revalidated; each ordered real contact, excluding self damage",
        },
    },
    HookDef {
        hook: HookName::OnTakeDamage,
        state: SupportState::Supported {
            entry: EntryPoint::DefenderCollisionQueue,
            evidence: "damage_feed pairs positive native percent commits with contacts and damage_dispatch revalidates both stocks before receiver-scoped PendingDamage delivery",
        },
    },
    HookDef {
        hook: HookName::OnChargeTick,
        state: SupportState::Supported {
            entry: EntryPoint::FrameTick,
            evidence: "timer_system::on_frame runs per-frame processing; charge accumulation evaluates there",
        },
    },
    HookDef {
        hook: HookName::OnTimeout,
        state: SupportState::Supported {
            entry: EntryPoint::TimerExpiry,
            evidence: "timer_system::schedule starts an authored countdown; a closed Cloud resource source may instead select the native Limit timer's evidenced standing-idle expiry, delivered on a distinct runtime hook",
        },
    },
    HookDef {
        hook: HookName::OnStockStart,
        state: SupportState::Supported {
            entry: EntryPoint::FighterSpawn,
            evidence: "initial GameEvent::Spawn plus agents::StatusEdgeTracker's exact same-identity transition into FIGHTER_STATUS_KIND_REBIRTH carry the fighter and stock generation",
        },
    },
    HookDef {
        hook: HookName::OnKo,
        state: SupportState::Supported {
            entry: EntryPoint::FighterDown,
            evidence: "agents::StatusEdgeTracker emits one identity-aware FighterDownObserved on the exact transition into FIGHTER_STATUS_KIND_DEAD; the Cloud matrix proved one firing per lost stock, rebirth re-arming, and no teardown firing",
        },
    },
    HookDef {
        hook: HookName::OnSpend,
        state: SupportState::StatusBound {
            entry: EntryPoint::StatusChange,
            evidence: "exact reviewed StatusChange bindings or the guarded grounded Cloud Limit Blade Beam consumption observation; the resource source rejects natural expiry and stock-loss clears",
        },
    },
    HookDef {
        hook: HookName::OnStatusEnter,
        state: SupportState::StatusBound {
            entry: EntryPoint::StatusChange,
            evidence: "the identity-aware per-frame status tracker emits exact previous/current status edges; this trigger reports entry into one registry-resolved fighter-owned status and makes no gameplay-semantic inference",
        },
    },
    HookDef {
        hook: HookName::OnStatusExit,
        state: SupportState::StatusBound {
            entry: EntryPoint::StatusChange,
            evidence: "the identity-aware per-frame status tracker emits exact previous/current status edges; this trigger reports exit from one registry-resolved fighter-owned status and makes no gameplay-semantic inference",
        },
    },
    HookDef { hook: HookName::OnArticleRemoved, state: SupportState::Supported { entry: EntryPoint::WeaponSpawn, evidence: "article_notifier retains exact owner and article kind for observed despawns" } },
    HookDef {
        hook: HookName::OnArticleSpawn,
        state: SupportState::Supported {
            entry: EntryPoint::WeaponSpawn,
            evidence: "article_notifier::on_weapon_frame observes new owned weapon lifetimes and emits GameEvent::ArticleSpawn; the binding accepts an optional exact article-kind filter",
        },
    },
    HookDef {
        hook: HookName::OnStore,
        state: SupportState::Unavailable {
            reason: "no store event exists; keeping a charge through a cancel is an input edge the plugin does not observe",
            unblocks: "an input-cancel feed, or a per-fighter stored-state binding from status research",
        },
    },
    HookDef {
        hook: HookName::OnWhiff,
        state: SupportState::Unavailable {
            reason: "no attack-resolution feed distinguishes a hit from a whiff",
            unblocks: "a hit-vs-whiff source on the attacker side",
        },
    },
    HookDef {
        hook: HookName::OnArticleCatch,
        state: SupportState::Unavailable {
            reason: "no catch feed exists; article ownership changes are not observed",
            unblocks: "an article ownership feed",
        },
    },
    HookDef {
        hook: HookName::OnArticleTimeout,
        state: SupportState::Unavailable {
            reason: "despawn is observed (article_notifier clears gone boids) but the cause — timeout, catch, or destroy — is indistinguishable",
            unblocks: "a despawn-cause feed",
        },
    },
    HookDef {
        hook: HookName::OnArticleTransfer,
        state: SupportState::Unavailable {
            reason: "no ownership-transfer feed exists",
            unblocks: "an article ownership feed",
        },
    },
    HookDef {
        hook: HookName::OnRng,
        state: SupportState::Unavailable {
            reason: "the IR is deterministic and the plugin exposes no random source for behavior",
            unblocks: "a constrained RNG design decision, not just a feed",
        },
    },
    HookDef {
        hook: HookName::OnCounterHit,
        state: SupportState::Unavailable {
            reason: "no counter-window feed exists",
            unblocks: "a counter-state feed from status research",
        },
    },
    HookDef {
        hook: HookName::OnPin,
        state: SupportState::Unavailable {
            reason: "vanilla pins are fighter-specific target statuses; no generic pin state exists",
            unblocks: "a generic target status that pins, proven in game",
        },
    },
    HookDef {
        hook: HookName::OnCarry,
        state: SupportState::Unavailable {
            reason: "carrying has no game event; a Holding opponent guard in an owned status already covers it",
            unblocks: "a game event for carrying that a guard cannot express",
        },
    },
    HookDef {
        hook: HookName::OnMash,
        state: SupportState::Unavailable {
            reason: "the Held opponent escape time value falls every frame, so a single mash press cannot be told apart yet",
            unblocks: "in-game proof of how much one mash press lowers the escape time",
        },
    },
    HookDef {
        hook: HookName::OnSwitch,
        state: SupportState::Supported {
            entry: EntryPoint::FrameTick,
            evidence: "outgoing down special followed by a different live same-entry form; bounded handoff cleared on KO/rebirth",
        },
    },
    HookDef {
        hook: HookName::OnAbsorb,
        state: SupportState::Supported {
            entry: EntryPoint::AttackerCollisionQueue,
            evidence: "13.0.4 per-contact absorber result at 0x3df93c, weapon source and fighter receiver; sealed receiver delivery",
        },
    },
];

/// Every registry row.
pub fn all() -> &'static [HookDef] {
    HOOKS
}

/// The registry row for a known hook.
pub fn lookup(hook: HookName) -> &'static HookDef {
    HOOKS
        .iter()
        .find(|def| def.hook == hook)
        .expect("HOOKS covers every HookName")
}

/// Parse a PLAN spelling into the closed vocabulary.
pub fn parse(raw: &str) -> Result<HookName, UnknownHook> {
    raw.parse()
}

/// Why a hook cannot be emitted: outside the vocabulary, or unavailable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookError {
    Unknown {
        name: String,
    },
    Unavailable {
        name: &'static str,
        reason: &'static str,
        unblocks: &'static str,
    },
}

impl fmt::Display for HookError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HookError::Unknown { name } => write!(
                formatter,
                "unknown hook {name:?}; hooks are the closed PLAN set, not free-form strings"
            ),
            HookError::Unavailable {
                name,
                reason,
                unblocks,
            } => write!(
                formatter,
                "hook {name} is unavailable: {reason} (unblocks: {unblocks})"
            ),
        }
    }
}

/// Resolve one hook for an emitted binding. Direct hooks need no additional
/// data; semantic hooks require the plan's reviewed status or resource trigger.
pub fn resolve_binding(
    raw: &str,
    has_validated_source: bool,
) -> Result<&'static HookDef, HookError> {
    let hook = parse(raw).map_err(|unknown| HookError::Unknown { name: unknown.name })?;
    let def = lookup(hook);
    match &def.state {
        SupportState::Supported { .. } => Ok(def),
        SupportState::StatusBound { .. } if has_validated_source => Ok(def),
        SupportState::StatusBound { .. } => Err(HookError::Unavailable {
            name: def.hook.name(),
            reason: "this hook requires a reviewed built-in resource source or an exact fighter-status trigger with recorded evidence and runtime validation",
            unblocks: "select a supported fighter resource source, or validate and record the exact status edge",
        }),
        SupportState::Unavailable { reason, unblocks } => Err(HookError::Unavailable {
            name: def.hook.name(),
            reason,
            unblocks,
        }),
    }
}

#[cfg(test)]
/// The F2.3 emission gate: parse the name and refuse anything that is not a
/// supported hook, naming why. Only `Ok` rows may be emitted.
pub fn resolve(raw: &str) -> Result<&'static HookDef, HookError> {
    resolve_binding(raw, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The PLAN hook set, spelled exactly as the plan spells it.
    const PLAN_HOOKS: [&str; 32] = [
        "onDealDamage",
        "onTakeDamage",
        "onAttackShielded",
        "onShieldHit",
        "onIncomingHit",
        "onReflect",
        "onGrab",
        "onThrow",
        "onGrabbed",
        "onItemPickup",
        "onItemThrown",
        "onChargeTick",
        "onStore",
        "onSpend",
        "onStatusEnter",
        "onStatusExit",
        "onTimeout",
        "onWhiff",
        "onStockStart",
        "onKO",
        "onArticleSpawn",
        "onArticleRemoved",
        "onArticleCatch",
        "onArticleTimeout",
        "onArticleTransfer",
        "onRNG",
        "onCounterHit",
        "onPin",
        "onCarry",
        "onMash",
        "onSwitch",
        "onAbsorb",
    ];

    #[test]
    fn unknown_names_are_rejected_with_the_name() {
        for raw in ["onHit", "", "ondealdamage", "onDealDamage "] {
            let error = parse(raw).unwrap_err();
            assert_eq!(error.name, raw);
            assert!(error.to_string().contains(raw));
        }
    }

    #[test]
    fn supported_hooks_resolve_with_cited_evidence() {
        for def in all() {
            if let SupportState::Supported { evidence, .. } = def.state {
                assert!(!evidence.is_empty(), "{:?} cites nothing", def.hook);
                let resolved = resolve(def.hook.name()).unwrap();
                assert_eq!(resolved.hook, def.hook);
            }
        }
        assert!(resolve(HookName::OnSpend.name()).is_err());
        assert!(resolve_binding(HookName::OnSpend.name(), true).is_ok());
    }

    #[test]
    fn unavailable_hooks_fail_closed_with_reason_and_unblocker() {
        for def in all() {
            if let SupportState::Unavailable { reason, unblocks } = def.state {
                assert!(!reason.is_empty() && !unblocks.is_empty());
                let error = resolve(def.hook.name()).unwrap_err();
                let message = error.to_string();
                assert!(
                    message.contains(def.hook.name())
                        && message.contains(reason)
                        && message.contains(unblocks),
                    "{message}"
                );
            }
        }
    }

    #[test]
    fn causal_damage_hooks_resolve_but_unclassified_hooks_stay_unavailable() {
        for hook in [HookName::OnDealDamage, HookName::OnTakeDamage] {
            assert!(resolve(hook.name()).is_ok());
        }
        for hook in [HookName::OnCounterHit, HookName::OnWhiff] {
            assert!(resolve(hook.name()).is_err());
        }
    }

    #[test]
    fn hook_names_round_trip_through_json() {
        assert_eq!(all().len(), PLAN_HOOKS.len());
        let mut seen = std::collections::BTreeSet::new();
        for def in all() {
            assert!(seen.insert(def.hook.name()), "duplicate hook");
        }
        for spelling in PLAN_HOOKS {
            let hook = parse(spelling).unwrap();
            let json = serde_json::to_string(&hook).unwrap();
            assert_eq!(json, format!("\"{spelling}\""));
            assert_eq!(serde_json::from_str::<HookName>(&json).unwrap(), hook);
        }
    }
}
