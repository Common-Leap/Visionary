//! Status-scoped damage for hitboxes created after the status callback.
use std::collections::BTreeMap;

pub struct Pending<I> {
    entries: BTreeMap<u32, (I, i32, [Option<f32>; 8])>,
}
impl<I: PartialEq> Default for Pending<I> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}
impl<I: PartialEq> Pending<I> {
    pub fn set(
        &mut self,
        boid: u32,
        identity: I,
        status: i32,
        id: u8,
        damage: f32,
    ) -> Result<(), String> {
        if id > 7 || !damage.is_finite() || !(0.0..=999.0).contains(&damage) {
            return Err("Invalid hitbox damage".into());
        }
        if !self.entries.contains_key(&boid) && self.entries.len() >= 8 {
            return Err("Hitbox damage actor limit reached".into());
        }
        if self.entries.get(&boid).is_some_and(|row| row.0 != identity) {
            self.entries.remove(&boid);
        }
        let row = self
            .entries
            .entry(boid)
            .or_insert_with(|| (identity, status, [None; 8]));
        if row.1 != status {
            row.1 = status;
            row.2 = [None; 8];
        }
        row.2[id as usize] = Some(damage);
        Ok(())
    }
    pub fn get(&self, boid: u32, identity: &I, status: i32, id: u8) -> Option<f32> {
        let row = self.entries.get(&boid)?;
        (row.0 == *identity && row.1 == status)
            .then(|| row.2.get(id as usize).copied().flatten())
            .flatten()
    }
    pub fn clear(&mut self, boid: u32) {
        self.entries.remove(&boid);
    }
}
