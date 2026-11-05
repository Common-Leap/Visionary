#[path = "../src/gimmicks/attack_damage.rs"]
mod attack_damage;
#[test]
fn pending_damage_is_exact_actor_status_and_hitbox_scoped() {
    let mut pending = attack_damage::Pending::default();
    pending.set(1, 10, 484, 0, 19.8).unwrap();
    assert_eq!(pending.get(1, &10, 484, 0), Some(19.8));
    for (boid, identity, status, id) in [
        (2, 10, 484, 0),
        (1, 11, 484, 0),
        (1, 10, 479, 0),
        (1, 10, 484, 1),
    ] {
        assert_eq!(pending.get(boid, &identity, status, id), None);
    }
    assert!(pending.set(1, 10, 484, 8, 1.0).is_err());
    assert!(pending.set(1, 10, 484, 0, f32::NAN).is_err());
    pending.clear(1);
    assert_eq!(pending.get(1, &10, 484, 0), None);
}
#[test]
fn replacement_lifetime_and_status_drop_old_hitbox_values() {
    let mut pending = attack_damage::Pending::default();
    pending.set(1, 10, 484, 0, 20.0).unwrap();
    pending.set(1, 11, 484, 1, 30.0).unwrap();
    assert_eq!(pending.get(1, &11, 484, 0), None);
    assert_eq!(pending.get(1, &11, 484, 1), Some(30.0));
    pending.set(1, 11, 479, 2, 10.0).unwrap();
    assert_eq!(pending.get(1, &11, 479, 1), None);
    for boid in 2..=8 {
        pending.set(boid, 11, 479, 0, 1.0).unwrap();
    }
    assert!(pending.set(9, 11, 479, 0, 1.0).is_err());
    pending.clear(1);
    pending.set(9, 11, 479, 0, 1.0).unwrap();
}
