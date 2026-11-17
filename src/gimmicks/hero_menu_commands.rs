//! Closed Hero command-selection helpers. Slots are native menu indices, not Work IDs.
pub const FEATURE: &str = "hero_menu.v1";

/// One of the four slots the native Command Selection menu offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum Command {
    Choose { slot: u8 },
}

impl Command {
    pub const SLOTS: u8 = 4;

    pub fn slot(self) -> u8 {
        let Self::Choose { slot } = self;
        slot
    }

    pub fn validate(self, fighter: &str) -> Result<(), String> {
        if fighter != "brave" {
            return Err("Hero menu helpers belong to Hero".into());
        }
        if self.slot() >= Self::SLOTS {
            return Err("Hero menu slot must be 0..=3".into());
        }
        Ok(())
    }
}
