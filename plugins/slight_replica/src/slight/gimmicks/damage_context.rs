//! Explicit contact guards shared by the static and owned engine adapters.

use super::{actor_identity::Identity, damage_dispatch::Event, native_inputs};

pub(super) fn input(event: Event) -> Result<native_inputs::DamageInput, String> {
    let direction = match event.direction() {
        super::damage_dispatch::Direction::Deal => native_inputs::DamageDirection::Deal,
        super::damage_dispatch::Direction::Take => native_inputs::DamageDirection::Take,
        super::damage_dispatch::Direction::AttackShielded => {
            native_inputs::DamageDirection::AttackShielded
        }
        super::damage_dispatch::Direction::ShieldHit => native_inputs::DamageDirection::ShieldHit,
    };
    match event.amount() {
        Some(amount) => native_inputs::DamageInput::new(direction, amount.value())?,
        None => native_inputs::DamageInput::shield(direction)?,
    }
    .with_projectile(event.contact().weapon.is_some())
    .with_receiver_status(
        event.receiver().identity.kind,
        event.receiver_status_at_contact(),
    )
}

/// The receiver check binds the interpreter's reservation to the exact actor
/// in the contact. The native feed check freshly resolves both participants,
/// generations and delivery fences; retained accessors are never dereferenced.
pub(super) fn is_current(event: Option<Event>, boid: u32, identity: Identity) -> bool {
    event.is_none_or(|event| {
        let receiver = event.receiver();
        receiver.identity.object_id == boid
            && receiver.identity.kind == identity.fighter_kind
            && receiver.identity.entry == identity.entry_id
            && receiver.identity.accessor == identity.module_accessor_addr
            && receiver.stock_generation == identity.stock_generation
            && super::damage_feed::event_is_current(event)
    })
}

pub(super) fn require_current(
    event: Option<Event>,
    boid: u32,
    identity: Identity,
) -> Result<(), String> {
    if is_current(event, boid, identity) {
        Ok(())
    } else {
        Err(
            "damage contact changed actor, stock, frame, configuration or capture completeness"
                .into(),
        )
    }
}
