//! Bounded requests delivered only at the target fighter's Main boundary.

pub(crate) struct Request<I> {
    pub boid: u32,
    pub fighter: String,
    pub identity: I,
    pub status: i32,
}

pub(crate) struct Pending<I> {
    slots: [Option<Request<I>>; 8],
}

impl<I> Default for Pending<I> {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
        }
    }
}

impl<I> Pending<I> {
    pub fn enqueue(&mut self, entry: i32, request: Request<I>) -> Result<(), String> {
        if !(0..8).contains(&entry) {
            return Err("fighter status entry is outside the eight fighter slots".into());
        }
        // Partners may share a player entry. Replace by exact BOID, never by
        // entry, so one fighter cannot consume another fighter's request.
        let slot = self
            .slots
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|r| r.boid == request.boid))
            .or_else(|| self.slots.iter().position(Option::is_none))
            .ok_or("pending fighter status actor limit exceeded")?;
        // Several authored requests for one object have deterministic last
        // request wins semantics, matching one native pending status slot.
        self.slots[slot] = Some(request);
        Ok(())
    }

    pub fn take(&mut self, boid: u32) -> Option<Request<I>> {
        self.slots
            .iter_mut()
            .find(|slot| slot.as_ref().is_some_and(|r| r.boid == boid))?
            .take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(boid: u32, identity: u32, status: i32) -> Request<u32> {
        Request {
            boid,
            identity,
            status,
            fighter: "cloud".into(),
        }
    }

    #[test]
    fn delivery_is_targeted_removed_before_reentry_and_never_retried() {
        let mut queue = Pending::default();
        queue.enqueue(0, request(10, 1, 20)).unwrap();
        queue.enqueue(1, request(11, 2, 21)).unwrap();
        assert!(queue.take(12).is_none());
        let delivered = queue.take(10).unwrap();
        assert_eq!(
            (
                delivered.identity,
                delivered.status,
                delivered.fighter.as_str()
            ),
            (1, 20, "cloud")
        );
        assert!(queue.take(10).is_none());
        assert_eq!(queue.take(11).unwrap().status, 21);
    }

    #[test]
    fn shared_entries_keep_distinct_actors_and_reused_boids_replace_old_requests() {
        let mut queue = Pending::default();
        queue.enqueue(0, request(10, 1, 20)).unwrap();
        queue.enqueue(0, request(12, 2, 21)).unwrap();
        queue.enqueue(1, request(12, 3, 22)).unwrap();
        assert_eq!(queue.take(10).unwrap().identity, 1);
        assert_eq!(queue.take(12).unwrap().identity, 3);
        assert!(queue.take(12).is_none());
        assert!(queue.enqueue(-1, request(1, 1, 1)).is_err());
        assert!(queue.enqueue(8, request(1, 1, 1)).is_err());
        for boid in 0..8 {
            queue.enqueue(0, request(boid, 1, 1)).unwrap();
        }
        assert!(queue.enqueue(0, request(8, 1, 1)).is_err());
        // Replacing a pending actor is possible even when the queue is full.
        queue.enqueue(0, request(0, 2, 2)).unwrap();
        assert_eq!(queue.take(0).unwrap().status, 2);
    }
}
