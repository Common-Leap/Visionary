//! Buildable managed ACMD adapter fixture.
//!
//! Visionary replaces this exact slot only after verifying the sibling runtime
//! contract. An empty installation is intentional: a later gimmick-only
//! export must not retain ACMD registrations from an earlier package.

const VISIONARY_STATIC_ACMD_ADAPTER_SLOT: &str = "visionary.gimmicks.generated-acmd-slot.v1";

/// Install ACMD registrations emitted by Visionary for this package.
pub fn install() {}
