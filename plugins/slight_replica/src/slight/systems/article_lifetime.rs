use std::collections::{HashMap, HashSet};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct LifetimeDelta {
    /// Objects that disappeared or changed accessor and must be reset first.
    pub reset: Vec<u32>,
    /// Current objects that were not already committed to the live set.
    pub candidates: Vec<(u32, u64)>,
    /// Current objects whose exact identity is unchanged.
    pub unchanged: Vec<(u32, u64)>,
}

/// Pure identity reconciliation for article/weapon lifetimes.
pub fn lifetime_delta(live: &HashMap<u32, u64>, current: &HashMap<u32, u64>) -> LifetimeDelta {
    let mut delta = LifetimeDelta::default();
    for (&boid, &old_accessor) in live {
        match current.get(&boid) {
            None => delta.reset.push(boid),
            Some(&accessor) if accessor != old_accessor => delta.reset.push(boid),
            Some(&accessor) => delta.unchanged.push((boid, accessor)),
        }
    }
    for (&boid, &accessor) in current {
        if live.get(&boid) != Some(&accessor) {
            delta.candidates.push((boid, accessor));
        }
    }
    delta.reset.sort_unstable();
    delta.candidates.sort_unstable();
    delta.unchanged.sort_unstable();
    delta
}

/// Commit only accepted candidates. Rejected candidates stay absent so the
/// next observation retries them; unchanged identities remain committed.
pub fn commit_lifetimes(delta: &LifetimeDelta, accepted: &[(u32, u64)]) -> HashMap<u32, u64> {
    let accepted: HashSet<_> = accepted.iter().copied().collect();
    let mut next_live: HashMap<u32, u64> = delta.unchanged.iter().copied().collect();
    for &(boid, accessor) in &delta.candidates {
        if accepted.contains(&(boid, accessor)) {
            next_live.insert(boid, accessor);
        }
    }
    next_live
}
