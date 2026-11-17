include!("../../plugins/slight_replica/src/slight/systems/article_lifetime.rs");

fn identities(entries: &[(u32, u64)]) -> HashMap<u32, u64> {
    entries.iter().copied().collect()
}

#[test]
fn same_accessor_does_not_dispatch_twice() {
    let live = identities(&[(17, 0x1000)]);
    let current = identities(&[(17, 0x1000)]);

    let delta = lifetime_delta(&live, &current);

    assert!(delta.reset.is_empty());
    assert!(delta.candidates.is_empty());
    assert_eq!(delta.unchanged, vec![(17, 0x1000)]);
    assert_eq!(commit_lifetimes(&delta, &[]), live);
}

#[test]
fn recycled_boid_with_new_accessor_dispatches_new_lifetime() {
    let live = identities(&[(17, 0x1000)]);
    let current = identities(&[(17, 0x2000)]);

    let delta = lifetime_delta(&live, &current);

    assert_eq!(delta.reset, vec![17]);
    assert_eq!(delta.candidates, vec![(17, 0x2000)]);
    assert!(delta.unchanged.is_empty());
    assert_eq!(commit_lifetimes(&delta, &delta.candidates), current);
}

#[test]
fn disappearance_then_reappearance_starts_a_new_lifetime() {
    let first_live = identities(&[(23, 0x3000)]);
    let disappeared = identities(&[]);

    let gone = lifetime_delta(&first_live, &disappeared);
    assert_eq!(gone.reset, vec![23]);
    let after_disappearance = commit_lifetimes(&gone, &[]);
    assert!(after_disappearance.is_empty());

    let reappeared = identities(&[(23, 0x3000)]);
    let born = lifetime_delta(&after_disappearance, &reappeared);
    assert_eq!(born.candidates, vec![(23, 0x3000)]);
    assert_eq!(commit_lifetimes(&born, &born.candidates), reappeared);
}

#[test]
fn rejected_owner_is_retried_until_accepted() {
    let live = identities(&[]);
    let current = identities(&[(31, 0x4000)]);

    let rejected = lifetime_delta(&live, &current);
    assert_eq!(rejected.candidates, vec![(31, 0x4000)]);
    let after_rejection = commit_lifetimes(&rejected, &[]);
    assert!(after_rejection.is_empty());

    let retried = lifetime_delta(&after_rejection, &current);
    assert_eq!(retried.candidates, vec![(31, 0x4000)]);
    assert_eq!(commit_lifetimes(&retried, &retried.candidates), current);
}
