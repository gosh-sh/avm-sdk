use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use crate::input::{decode_batch_input, decode_message_input};
use crate::output::{encode_batch_output_for, encode_message_output_for};
use crate::{BatchContract, Contract};

/// Bounded guest-owned storage used for the live AVM input/output allocations.
///
/// AVM invokes one entrypoint on a fresh instance, then frees output followed
/// by input. That fixed LIFO ownership rule lets this arena expose the raw WASM
/// pointer ABI while all Rust access remains bounds-checked and safe.
#[doc(hidden)]
pub struct GuestArena<const N: usize> {
    bytes: [AtomicU8; N],
    next: AtomicU32,
    allocation_starts: [AtomicU32; 2],
    allocation_lengths: [AtomicU32; 2],
    allocation_count: AtomicU8,
}

impl<const N: usize> GuestArena<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [const { AtomicU8::new(0) }; N],
            next: AtomicU32::new(0),
            allocation_starts: [const { AtomicU32::new(0) }; 2],
            allocation_lengths: [const { AtomicU32::new(0) }; 2],
            allocation_count: AtomicU8::new(0),
        }
    }

    pub fn allocate(&self, len: u32) -> u32 {
        if len == 0 {
            return 0;
        }
        let allocation_count = usize::from(self.allocation_count.load(Ordering::Relaxed));
        if allocation_count >= self.allocation_starts.len() {
            return 0;
        }
        let start = self.next.load(Ordering::Relaxed);
        let Some(end) = start.checked_add(len) else {
            return 0;
        };
        let Ok(capacity) = u32::try_from(N) else {
            return 0;
        };
        if end > capacity {
            return 0;
        }
        let Some(pointer) = self.base_pointer().and_then(|base| base.checked_add(start)) else {
            return 0;
        };
        if pointer == 0 {
            return 0;
        }
        self.allocation_starts[allocation_count].store(start, Ordering::Relaxed);
        self.allocation_lengths[allocation_count].store(len, Ordering::Relaxed);
        self.next.store(end, Ordering::Relaxed);
        self.allocation_count.store((allocation_count + 1) as u8, Ordering::Relaxed);
        pointer
    }

    pub fn free(&self, ptr: u32, len: u32) -> bool {
        let Some(start) = self.offset(ptr) else {
            return false;
        };
        let allocation_count = usize::from(self.allocation_count.load(Ordering::Relaxed));
        let Some(slot) = allocation_count.checked_sub(1) else {
            return false;
        };
        if self.allocation_starts[slot].load(Ordering::Relaxed) != start
            || self.allocation_lengths[slot].load(Ordering::Relaxed) != len
        {
            return false;
        }
        let Some(end) = start.checked_add(len) else {
            return false;
        };
        if len == 0 || self.next.load(Ordering::Relaxed) != end {
            return false;
        }
        self.next.store(start, Ordering::Relaxed);
        self.allocation_count.store(slot as u8, Ordering::Relaxed);
        true
    }

    fn read(&self, ptr: u32, len: u32) -> Option<Vec<u8>> {
        let start = usize::try_from(self.offset(ptr)?).ok()?;
        let length = usize::try_from(len).ok()?;
        let end = start.checked_add(length)?;
        if end > usize::try_from(self.next.load(Ordering::Relaxed)).ok()? {
            return None;
        }
        let source = self.bytes.get(start..end)?;
        let mut bytes = Vec::with_capacity(length);
        bytes.extend(source.iter().map(|byte| byte.load(Ordering::Relaxed)));
        Some(bytes)
    }

    fn write_output(&self, bytes: &[u8]) -> u64 {
        let Ok(len) = u32::try_from(bytes.len()) else {
            return 0;
        };
        let ptr = self.allocate(len);
        if ptr == 0 {
            return 0;
        }
        let Some(start) = self.offset(ptr).and_then(|offset| usize::try_from(offset).ok()) else {
            return 0;
        };
        let Some(destination) = self.bytes.get(start..start.saturating_add(bytes.len())) else {
            return 0;
        };
        for (destination, source) in destination.iter().zip(bytes) {
            destination.store(*source, Ordering::Relaxed);
        }
        (u64::from(ptr) << 32) | u64::from(len)
    }

    fn base_pointer(&self) -> Option<u32> {
        u32::try_from(self.bytes.as_ptr() as usize).ok()
    }

    fn offset(&self, ptr: u32) -> Option<u32> {
        ptr.checked_sub(self.base_pointer()?)
            .filter(|offset| usize::try_from(*offset).ok().is_some_and(|offset| offset < N))
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
    let Some(input_bytes) = arena.read(input_ptr, input_len) else {
        return 0;
    };
    let Ok(input) = decode_message_input(&input_bytes) else {
        return 0;
    };
    if input.message.is_deploy() != is_init {
        return 0;
    }
    let output = if is_init { T::init(input) } else { T::process_message(input) };
    let Ok(output_bytes) = encode_message_output_for(&input, is_init, &output) else {
        return 0;
    };
    arena.write_output(&output_bytes)
}

#[doc(hidden)]
pub fn dispatch_batch<T: BatchContract, const N: usize>(
    arena: &GuestArena<N>,
    input_ptr: u32,
    input_len: u32,
) -> u64 {
    let Some(input_bytes) = arena.read(input_ptr, input_len) else {
        return 0;
    };
    let Ok(input) = decode_batch_input(&input_bytes) else {
        return 0;
    };
    let output = T::process_batch(input);
    let Ok(output_bytes) = encode_batch_output_for(&input, &output) else {
        return 0;
    };
    arena.write_output(&output_bytes)
}
