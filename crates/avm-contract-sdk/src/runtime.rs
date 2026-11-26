//! Guest arena behind the `avm_guest_alloc` / `avm_guest_free` exports.
//!
//! The module exists only on WASM, where the AVM host calls the exports.
//! Unsafe code: the heap buffer of an input allocation is not zero-filled.
//! The host copies the whole input into it after `avm_guest_alloc` and before the
//! entrypoint (AVM executor call flow), so [`host_written`] is the one place that
//! views those bytes as `&[u8]`. Contract code runs only inside an entrypoint
//! dispatch, and a dispatch started while another runs is refused, so contract
//! code cannot reach the view on a buffer the host did not write. The invariant,
//! the reason it holds and the WASM tests that check it are listed in the SDK
//! `docs/unsafe.md`.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::mem::MaybeUninit;
use core::ops::Range;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::input::{decode_batch_input, decode_message_input};
use crate::output::{
    batch_output_len_for, message_output_len_for, write_batch_output, write_message_output,
    EncodeError, OutputWriter,
};
#[cfg(feature = "system-mint")]
use crate::output::{mint_output_len_for, write_mint_output};
#[cfg(feature = "system-mint")]
use crate::MintContract;
use crate::{BatchContract, Contract};

/// Static bytes that serve small allocations without growing memory.
const STATIC_BYTES: usize = 4096;

/// Bounded guest-owned storage used for the live AVM input/output allocations.
///
/// AVM invokes one entrypoint on a fresh instance, then frees output followed
/// by input. At most two allocations are live, in LIFO order, and their total
/// length never exceeds `N`. An allocation that fits the small static region
/// stays there; a larger one is a heap buffer taken in the export call that
/// makes it, so `N` bounds live bytes without reserving them in initial memory.
///
/// The host writes and reads guest memory only between export calls, at
/// addresses this arena returned. Rust code reaches the bytes only through the
/// lock while an export call runs, so no Rust reference observes a host write.
#[doc(hidden)]
pub struct GuestArena<const N: usize> {
    state: Mutex<ArenaState>,
}

struct ArenaState {
    region: [u8; STATIC_BYTES],
    /// Live allocations in LIFO order.
    live: [Allocation; 2],
}

/// `Free` holds no pointer and is tag 0, so the initial arena is
/// zero-initialized memory and adds no data bytes to the module. The explicit
/// tag keeps it so: without it the `Vec` capacity niche would encode `Free`
/// with non-zero bytes.
#[repr(u8)]
enum Allocation {
    Free,
    Static(Range<usize>),
    Heap(Box<[u8]>),
    /// A heap input buffer that the host writes before the entrypoint reads it:
    /// the first `len` bytes of the reserved capacity, never initialized by
    /// the guest.
    Input(Vec<u8>, usize),
}

impl Allocation {
    fn len(&self) -> usize {
        match self {
            Self::Free => 0,
            Self::Static(range) => range.len(),
            Self::Heap(bytes) => bytes.len(),
            Self::Input(_, len) => *len,
        }
    }
}

impl<const N: usize> GuestArena<N> {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(ArenaState {
                region: [0; STATIC_BYTES],
                live: [Allocation::Free, Allocation::Free],
            }),
        }
    }

    pub fn allocate(&self, len: u32) -> u32 {
        // The host writes every input byte before the entrypoint reads it, so a
        // heap input buffer is not zero-filled (see `host_written`).
        self.insert(len, None::<fn(ArenaWriter<'_>) -> Result<(), EncodeError>>)
    }

    pub fn free(&self, ptr: u32, len: u32) -> bool {
        let mut state = self.lock();
        let Some(slot) = state.live.iter().rposition(|allocation| allocation.len() != 0) else {
            return false;
        };
        if address(&mut state, slot) != Some(ptr)
            || usize::try_from(len) != Ok(state.live[slot].len())
        {
            return false;
        }
        state.live[slot] = Allocation::Free;
        true
    }

    /// Registers exactly `len` bytes produced by `fill` as the next live
    /// allocation, or an input allocation for the host when `fill` is `None`.
    /// Heap memory is requested only after the capacity check.
    fn insert<F: FnOnce(ArenaWriter<'_>) -> Result<(), EncodeError>>(
        &self,
        len: u32,
        fill: Option<F>,
    ) -> u32 {
        let Ok(length) = usize::try_from(len) else {
            return 0;
        };
        let mut guard = self.lock();
        let state = &mut *guard;
        let Some(slot) = state.live.iter().position(|allocation| allocation.len() == 0) else {
            return 0;
        };
        let live = state.live.iter().map(Allocation::len).sum::<usize>();
        if length == 0 || live.checked_add(length).is_none_or(|end| end > N) {
            return 0;
        }
        let start = match &state.live[..slot] {
            [Allocation::Static(range)] => range.end,
            _ => 0,
        };
        let range = start..start.saturating_add(length);
        let allocation = if let Some(region) = state.region.get_mut(range.clone()) {
            if fill.is_some_and(|fill| fill(ArenaWriter::Static(region)).is_err()) {
                return 0;
            }
            Allocation::Static(range)
        } else if let Some(fill) = fill {
            let mut bytes = Vec::new();
            if bytes.try_reserve_exact(length).is_err()
                || fill(ArenaWriter::Heap(&mut bytes, length)).is_err()
                || bytes.len() != length
            {
                return 0;
            }
            Allocation::Heap(bytes.into_boxed_slice())
        } else {
            let mut bytes = Vec::new();
            if bytes.try_reserve_exact(length).is_err() {
                return 0;
            }
            Allocation::Input(bytes, length)
        };
        state.live[slot] = allocation;
        address(state, slot).unwrap_or_else(|| {
            state.live[slot] = Allocation::Free;
            0
        })
    }

    fn write_output(
        &self,
        length: usize,
        encode: impl FnOnce(&mut ArenaWriter<'_>) -> Result<(), EncodeError>,
    ) -> u64 {
        let Ok(len) = u32::try_from(length) else {
            return 0;
        };
        match self.insert(
            len,
            Some(|mut writer: ArenaWriter<'_>| {
                encode(&mut writer)?;
                writer.finish()
            }),
        ) {
            0 => 0,
            ptr => (u64::from(ptr) << 32) | u64::from(len),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ArenaState> {
        // Contracts abort on panic, so no export call observes a poisoned lock.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The guest address of a live allocation.
fn address(state: &mut ArenaState, slot: usize) -> Option<u32> {
    let pointer = match &mut state.live[slot] {
        Allocation::Free => return None,
        Allocation::Static(range) => state.region.as_mut_ptr() as usize + range.start,
        Allocation::Heap(bytes) => bytes.as_mut_ptr() as usize,
        Allocation::Input(bytes, _) => bytes.as_mut_ptr() as usize,
    };
    u32::try_from(pointer).ok().filter(|address| *address != 0)
}

/// One in-bounds range of a live allocation.
fn live_range(state: &mut ArenaState, ptr: u32, len: u32) -> Option<&[u8]> {
    let length = usize::try_from(len).ok()?;
    let (slot, range) = (0..state.live.len()).find_map(|slot| {
        let offset = usize::try_from(ptr.checked_sub(address(state, slot)?)?).ok()?;
        let end = offset.checked_add(length)?;
        let live = state.live[slot].len();
        (live != 0 && end <= live).then_some((slot, offset..end))
    })?;
    match &mut state.live[slot] {
        Allocation::Free => None,
        Allocation::Static(region) => {
            state.region.get(region.start + range.start..region.start + range.end)
        }
        Allocation::Heap(bytes) => bytes.get(range),
        Allocation::Input(bytes, _) => bytes.spare_capacity_mut().get(range).map(host_written),
    }
}

/// Views a range of a heap input allocation as the bytes the host wrote.
///
/// Contract: `bytes` lies inside an `Allocation::Input` buffer returned by
/// `avm_guest_alloc`, and is read only by an entrypoint call for that input.
/// The AVM executor call flow (SDK `docs/unsafe.md`: `avm_guest_alloc(input.len)`,
/// copy of the whole input into guest memory, then the entrypoint) writes every
/// requested byte before the entrypoint runs, and the guest writes nothing into
/// the buffer itself. Contract code cannot start a dispatch of its own on an
/// unwritten buffer: [`Dispatch`] refuses a dispatch inside another.
#[allow(unsafe_code)]
fn host_written(bytes: &[MaybeUninit<u8>]) -> &[u8] {
    // SAFETY: by the AVM executor call flow (docs/unsafe.md) the host copies the
    // whole input into this allocation after `avm_guest_alloc` returns and before
    // it calls the entrypoint, the only caller of this view, so every byte of
    // `bytes` is initialized. Contract code runs only inside an entrypoint
    // dispatch and a dispatch it starts is refused before this view (`Dispatch`).
    // `MaybeUninit<u8>` has the size, alignment and layout of `u8`, and the
    // returned slice borrows `bytes` with its length.
    unsafe { &*(bytes as *const [MaybeUninit<u8>] as *const [u8]) }
}

std::thread_local! {
    // Set while an entrypoint dispatch runs.
    static DISPATCHING: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

/// One running entrypoint dispatch. Contract code runs only inside a dispatch,
/// so refusing a second one keeps contract code from viewing an input buffer
/// that the host did not write (see `host_written`).
struct Dispatch;

impl Dispatch {
    fn enter() -> Option<Self> {
        // The guard exists only on entry: dropping a refused one would clear the flag.
        if DISPATCHING.with(|running| running.replace(true)) {
            return None;
        }
        Some(Self)
    }
}

impl Drop for Dispatch {
    fn drop(&mut self) {
        DISPATCHING.with(|running| running.set(false));
    }
}

/// The destination of one output allocation.
enum ArenaWriter<'a> {
    Static(&'a mut [u8]),
    Heap(&'a mut Vec<u8>, usize),
}

impl ArenaWriter<'_> {
    fn finish(self) -> Result<(), EncodeError> {
        let complete = match self {
            Self::Static(remaining) => remaining.is_empty(),
            Self::Heap(bytes, length) => bytes.len() == length,
        };
        if complete {
            Ok(())
        } else {
            Err(EncodeError::LengthOverflow)
        }
    }
}

impl OutputWriter for ArenaWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), EncodeError> {
        match self {
            Self::Static(remaining) => {
                let (destination, rest) = core::mem::take(remaining)
                    .split_at_mut_checked(bytes.len())
                    .ok_or(EncodeError::LengthOverflow)?;
                destination.copy_from_slice(bytes);
                *remaining = rest;
            }
            Self::Heap(buffer, length) => {
                if bytes.len() > *length - buffer.len() {
                    return Err(EncodeError::LengthOverflow);
                }
                buffer.extend_from_slice(bytes);
            }
        }
        Ok(())
    }
}

impl<const N: usize> Default for GuestArena<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[doc(hidden)]
pub fn dispatch_init<T: Contract, const N: usize>(
    arena: &GuestArena<N>,
    input_ptr: u32,
    input_len: u32,
) -> u64 {
    dispatch_message_inner::<T, N>(arena, input_ptr, input_len, true)
}

#[doc(hidden)]
pub fn dispatch_message<T: Contract, const N: usize>(
    arena: &GuestArena<N>,
    input_ptr: u32,
    input_len: u32,
) -> u64 {
    dispatch_message_inner::<T, N>(arena, input_ptr, input_len, false)
}

fn dispatch_message_inner<T: Contract, const N: usize>(
    arena: &GuestArena<N>,
    input_ptr: u32,
    input_len: u32,
    is_init: bool,
) -> u64 {
    let Some(_dispatch) = Dispatch::enter() else {
        return 0;
    };
    let (output, output_len) = {
        let mut state = arena.lock();
        let Some(input_bytes) = live_range(&mut state, input_ptr, input_len) else {
            return 0;
        };
        let Ok(input) = decode_message_input(input_bytes) else {
            return 0;
        };
        if input.message.is_deploy() != is_init {
            return 0;
        }
        let output = if is_init { T::init(input) } else { T::process_message(input) };
        let Ok(output_len) = message_output_len_for(&input, is_init, &output) else {
            return 0;
        };
        (output, output_len)
    };
    arena.write_output(output_len, |writer| write_message_output(writer, &output))
}

#[cfg(feature = "system-mint")]
#[doc(hidden)]
/// `process_message` of a system minter ([`MintContract`]): the ordinary
/// message dispatch, with the mint intent the contract returns.
pub fn dispatch_mint_message<T: MintContract, const N: usize>(
    arena: &GuestArena<N>,
    input_ptr: u32,
    input_len: u32,
) -> u64 {
    let Some(_dispatch) = Dispatch::enter() else {
        return 0;
    };
    let (output, mints, output_len) = {
        let mut state = arena.lock();
        let Some(input_bytes) = live_range(&mut state, input_ptr, input_len) else {
            return 0;
        };
        let Ok(input) = decode_message_input(input_bytes) else {
            return 0;
        };
        if input.message.is_deploy() {
            return 0;
        }
        let (output, mints) = T::process_mint_message(input);
        let Ok(output_len) = mint_output_len_for(&input, &output, &mints) else {
            return 0;
        };
        (output, mints, output_len)
    };
    arena.write_output(output_len, |writer| write_mint_output(writer, &output, &mints))
}

pub fn dispatch_batch<T: BatchContract, const N: usize>(
    arena: &GuestArena<N>,
    input_ptr: u32,
    input_len: u32,
) -> u64 {
    let Some(_dispatch) = Dispatch::enter() else {
        return 0;
    };
    let (output, output_len) = {
        let mut state = arena.lock();
        let Some(input_bytes) = live_range(&mut state, input_ptr, input_len) else {
            return 0;
        };
        let Ok(input) = decode_batch_input(input_bytes) else {
            return 0;
        };
        let output = T::process_batch(input);
        let Ok(output_len) = batch_output_len_for(&input, &output) else {
            return 0;
        };
        (output, output_len)
    };
    arena.write_output(output_len, |writer| write_batch_output(writer, &output))
}
