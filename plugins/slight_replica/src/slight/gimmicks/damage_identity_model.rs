//! Fixed-capacity, engine-independent identity snapshots for collision diagnostics.
//!
//! This model only records and compares object identity. It does not assign
//! attacker/defender roles or infer damage, shield, item, or gameplay meaning.

pub const IDENTITY_CAPACITY: usize = 64;
pub const FIGHTER_ENTRY_COUNT: i32 = 8;
pub const MAX_DIAGNOSTIC_ENTRY_ID: i32 = 63;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub object_id: u32,
    /// Numeric address only. Never dereferenced by this model.
    pub module_accessor: u64,
    pub category: i32,
    pub kind: i32,
    pub entry_id: i32,
    pub owner_entry: Option<i32>,
    pub stock_generation: Option<u32>,
}

impl Identity {
    pub const ZERO: Self = Self {
        object_id: 0,
        module_accessor: 0,
        category: 0,
        kind: 0,
        entry_id: 0,
        owner_entry: None,
        stock_generation: None,
    };

    /// Only fighter-category records with a real player entry are fighters.
    /// Other categories remain available as diagnostic identities.
    pub const fn is_fighter(self) -> bool {
        self.category == 0 && self.entry_id >= 0 && self.entry_id < FIGHTER_ENTRY_COUNT
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PublishedIdentity {
    pub identity: Identity,
    pub epoch: u64,
    pub publication_frame: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplaceError {
    CapacityExceeded,
    DuplicateObjectId(u32),
    InvalidModuleAccessor(u32),
    InvalidCategory(u32),
    InvalidKind(u32),
    InvalidEntryId(u32),
    InvalidOwnerEntry(u32),
}

#[derive(Clone, Copy, Debug)]
pub struct IdentityTable {
    epoch: u64,
    publication_frame: u64,
    rows: [Identity; IDENTITY_CAPACITY],
    len: usize,
}

impl Default for IdentityTable {
    fn default() -> Self {
        Self::new()
    }
}

impl IdentityTable {
    pub const fn new() -> Self {
        Self {
            epoch: 0,
            publication_frame: 0,
            rows: [Identity::ZERO; IDENTITY_CAPACITY],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    pub const fn publication_frame(&self) -> u64 {
        self.publication_frame
    }

    /// Atomically replace the publication from an iterator of already-copied
    /// identities. Any malformed or ambiguous publication clears the previous
    /// table so readers can never fall back to stale rows.
    pub fn replace<I>(&mut self, epoch: u64, frame: u64, identities: I) -> Result<(), ReplaceError>
    where
        I: IntoIterator<Item = Identity>,
    {
        let mut staged = [Identity::ZERO; IDENTITY_CAPACITY];
        let mut staged_len = 0;

        for identity in identities {
            if staged_len == IDENTITY_CAPACITY {
                self.clear();
                return Err(ReplaceError::CapacityExceeded);
            }
            if let Err(error) = validate(identity) {
                self.clear();
                return Err(error);
            }
            if staged[..staged_len]
                .iter()
                .any(|seen| seen.object_id == identity.object_id)
            {
                self.clear();
                return Err(ReplaceError::DuplicateObjectId(identity.object_id));
            }
            staged[staged_len] = identity;
            staged_len += 1;
        }

        self.rows = staged;
        self.len = staged_len;
        self.epoch = epoch;
        self.publication_frame = frame;
        Ok(())
    }

    /// Return an identity only for the exact epoch and publication frame.
    pub fn lookup(
        &self,
        epoch: u64,
        capture_frame: u64,
        object_id: u32,
    ) -> Option<PublishedIdentity> {
        if epoch != self.epoch || capture_frame != self.publication_frame {
            return None;
        }
        self.rows[..self.len]
            .iter()
            .copied()
            .find(|identity| identity.object_id == object_id)
            .map(|identity| PublishedIdentity {
                identity,
                epoch: self.epoch,
                publication_frame: self.publication_frame,
            })
    }

    /// Discard every row and reset publication metadata.
    pub fn clear(&mut self) {
        self.rows = [Identity::ZERO; IDENTITY_CAPACITY];
        self.len = 0;
        self.epoch = 0;
        self.publication_frame = 0;
    }
}

fn validate(identity: Identity) -> Result<(), ReplaceError> {
    if identity.module_accessor == 0 {
        return Err(ReplaceError::InvalidModuleAccessor(identity.object_id));
    }
    if identity.category < 0 {
        return Err(ReplaceError::InvalidCategory(identity.object_id));
    }
    if identity.kind < 0 {
        return Err(ReplaceError::InvalidKind(identity.object_id));
    }
    if identity.entry_id < 0 || identity.entry_id > MAX_DIAGNOSTIC_ENTRY_ID {
        return Err(ReplaceError::InvalidEntryId(identity.object_id));
    }
    if identity.category == 0 && identity.entry_id >= FIGHTER_ENTRY_COUNT {
        return Err(ReplaceError::InvalidEntryId(identity.object_id));
    }
    if identity
        .owner_entry
        .is_some_and(|entry| !(0..FIGHTER_ENTRY_COUNT).contains(&entry))
    {
        return Err(ReplaceError::InvalidOwnerEntry(identity.object_id));
    }
    Ok(())
}
