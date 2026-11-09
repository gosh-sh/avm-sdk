//! The owner key a contract keeps in its own data.
//!
//! A contract that accepts ordinary external calls declares
//! `owner_key_offset` in the `[descriptor]` of its `avm-contract.toml`. The
//! node then admits an external call only when it is signed by the 32 bytes at
//! that offset of the account data. The protocol never writes these bytes:
//! `init` writes the initial key, by convention the deploy's `owner_id`
//! ([`crate::DeployInput::owner_id`]), and any later call may rotate it. A key
//! written by a call authorizes external calls from the next block.

use crate::EncodeError;

/// Length of the owner key in the account data.
pub const OWNER_KEY_BYTES: usize = 32;

/// Reads the owner key at `owner_key_offset` of `data`, as external admission
/// reads it: `None` when the 32 bytes do not fit `data` or are all zero.
pub fn owner_key(data: &[u8], owner_key_offset: u32) -> Option<[u8; OWNER_KEY_BYTES]> {
    let key: [u8; OWNER_KEY_BYTES] = data.get(slot(owner_key_offset)?)?.try_into().ok()?;
    (key != [0; OWNER_KEY_BYTES]).then_some(key)
}

/// Writes `key` at `owner_key_offset` of `data`: `init` installs the initial
/// key this way and a later call rotates it. A zero key leaves the account
/// without an owner key, so every ordinary external call to it is rejected.
/// Returns `InvalidIndex`, with `data` unchanged, when the 32 bytes do not fit.
pub fn set_owner_key(
    data: &mut [u8],
    owner_key_offset: u32,
    key: [u8; OWNER_KEY_BYTES],
) -> Result<(), EncodeError> {
    let target = slot(owner_key_offset)
        .and_then(|range| data.get_mut(range))
        .ok_or(EncodeError::InvalidIndex)?;
    target.copy_from_slice(&key);
    Ok(())
}

fn slot(owner_key_offset: u32) -> Option<core::ops::Range<usize>> {
    let start = usize::try_from(owner_key_offset).ok()?;
    Some(start..start.checked_add(OWNER_KEY_BYTES)?)
}
