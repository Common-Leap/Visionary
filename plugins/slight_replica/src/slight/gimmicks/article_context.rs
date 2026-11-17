//! Dispatch-local owned article payload, fenced by the observed owner identity.
use super::actor_identity::Identity;
use std::cell::RefCell;
thread_local! { static CURRENT: RefCell<Option<(u32, Identity, bool, String)>> = const { RefCell::new(None) }; }

pub(crate) fn with_event<R>(
    boid: u32,
    identity: Identity,
    spawned: bool,
    symbol: &str,
    run: impl FnOnce() -> R,
) -> R {
    struct Restore(Option<(u32, Identity, bool, String)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|slot| slot.replace(self.0.take()));
        }
    }
    let _restore =
        Restore(CURRENT.with(|slot| slot.replace(Some((boid, identity, spawned, symbol.into())))));
    run()
}

pub(crate) fn sample(boid: u32, expected: Identity) -> Option<(bool, String)> {
    CURRENT.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|(owner, identity, _, _)| *owner == boid && *identity == expected)
            .map(|(_, _, spawned, symbol)| (*spawned, symbol.clone()))
    })
}

/// Resolve “my active article” through the live ownership and weapon-kind fence.
/// Resolve on each dispatch so a recycled battle-object id is never persisted.
pub(crate) fn newest_owned_article(owner: u32, fighter: &str, symbol: &str) -> Option<u32> {
    use super::registry::{self, IdentifierKind};
    registry::resolve(fighter, symbol, IdentifierKind::Article).ok()?;
    let owned = crate::slight::systems::article_notifier::owned_weapons(owner, fighter, symbol);
    crate::slight::agents::newest_registered(&owned)
}
