use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ops::Range;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::input::{decode_batch_input, decode_message_input};
use crate::output::{
    batch_output_len_for, message_output_len_for, write_batch_output, write_message_output,
    EncodeError, OutputWriter,
};
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

/// `Free` holds no pointer, so the initial arena is zero-initialized memory and
/// adds no data bytes to the module.
enum Allocation {
    Free,
    Static(Range<usize>),
    Heap(Box<[u8]>),
}

impl Allocation {
    fn len(&self) -> usize {
        match self {
            Self::Free => 0,
            Self::Static(range) => range.len(),
            Self::Heap(bytes) => bytes.len(),
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
        // The host overwrites every byte; a heap buffer only needs initialization.
        self.insert(len, |writer| {
            if let ArenaWriter::Heap(bytes, length) = writer {
                bytes.resize(length, 0);
            }
            Ok(())
        })
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
    /// allocation. Heap memory is requested only after the capacity check.
    fn insert(
        &self,
        len: u32,
        fill: impl FnOnce(ArenaWriter<'_>) -> Result<(), EncodeError>,
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
            if fill(ArenaWriter::Static(region)).is_err() {
                return 0;
            }
            Allocation::Static(range)
        } else {
            let mut bytes = Vec::new();
            if bytes.try_reserve_exact(length).is_err()
                || fill(ArenaWriter::Heap(&mut bytes, length)).is_err()
                || bytes.len() != length
            {
                return 0;
            }
            Allocation::Heap(bytes.into_boxed_slice())
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
        match self.insert(len, |mut writer| {
            encode(&mut writer)?;
            writer.finish()
        }) {
            0 => 0,
            ptr => (u64::from(ptr) << 32) | u64::from(len),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ArenaState> {
        // Contracts abort on panic, so no export call observes a poisoned lock.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The guest address of a live allocation. Native unit tests use logical WASM
/// addresses that follow the earlier allocation and never dereference them.
fn address(state: &mut ArenaState, slot: usize) -> Option<u32> {
    #[cfg(test)]
    return u32::try_from(state.live[..slot].iter().map(Allocation::len).sum::<usize>() + 1).ok();
    #[cfg(not(test))]
    let pointer = match &mut state.live[slot] {
        Allocation::Free => return None,
        Allocation::Static(range) => state.region.as_mut_ptr() as usize + range.start,
        Allocation::Heap(bytes) => bytes.as_mut_ptr() as usize,
    };
    #[cfg(not(test))]
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
    match &state.live[slot] {
        Allocation::Free => None,
        Allocation::Static(region) => {
            state.region.get(region.start + range.start..region.start + range.end)
        }
        Allocation::Heap(bytes) => bytes.get(range),
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
        #[cfg(test)]
        crate::output::work::arena_write(bytes.len());
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

#[doc(hidden)]
pub fn dispatch_batch<T: BatchContract, const N: usize>(
    arena: &GuestArena<N>,
    input_ptr: u32,
    input_len: u32,
) -> u64 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MessageInput, MessageOutput, StateUpdate};
    use alloc::vec;

    struct Echo;
    impl Contract for Echo {
        fn init(input: MessageInput<'_>) -> MessageOutput {
            Self::process_message(input)
        }

        fn process_message(input: MessageInput<'_>) -> MessageOutput {
            MessageOutput::success(StateUpdate::FullReplace(input.state.to_vec()))
        }
    }

    fn input() -> Vec<u8> {
        // Canonical ACMI with one internal message, no extras or optional sections.
        let mut bytes = vec![0; 262];
        bytes[..4].copy_from_slice(b"ACMI");
        bytes[60..124].fill(1);
        bytes[156..220].fill(2);
        bytes[220..252].fill(3);
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(b"old");
        bytes.extend_from_slice(&176u32.to_le_bytes());
        let mut message = vec![0; 176];
        message[8..16].copy_from_slice(&1u64.to_le_bytes());
        message[48..112].fill(1);
        message[112..176].fill(2);
        bytes.extend_from_slice(&message);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        decode_message_input(&bytes).unwrap();
        bytes
    }

    impl<const N: usize> GuestArena<N> {
        fn read(&self, ptr: u32, len: u32) -> Option<Vec<u8>> {
            live_range(&mut self.lock(), ptr, len).map(<[u8]>::to_vec)
        }

        /// Live allocation count and their total bytes.
        fn live(&self) -> (usize, usize) {
            let state = self.lock();
            let count = state.live.iter().filter(|allocation| allocation.len() != 0).count();
            (count, state.live.iter().map(Allocation::len).sum())
        }
    }

    /// The backing bytes of one live allocation.
    fn backing(state: &mut ArenaState, slot: usize) -> &mut [u8] {
        match &mut state.live[slot] {
            Allocation::Free => &mut [],
            Allocation::Static(range) => &mut state.region[range.clone()],
            Allocation::Heap(bytes) => &mut bytes[..],
        }
    }

    fn load<const N: usize>(arena: &GuestArena<N>, bytes: &[u8]) -> u32 {
        let ptr = arena.allocate(bytes.len() as u32);
        assert_ne!(ptr, 0);
        // The host writes the input between export calls.
        let mut state = arena.lock();
        let slot = state.live.iter().rposition(|allocation| allocation.len() != 0).unwrap();
        backing(&mut state, slot).copy_from_slice(bytes);
        ptr
    }

    #[test]
    fn input_view_borrows_live_bytes_and_bounds_at_chunk_boundaries() {
        let arena = GuestArena::<66_016>::new();
        for len in [1, 7, 8, 9, 15, 16, 17, 65_536, 66_006] {
            let bytes = (0..len).map(|index| (index % 251) as u8).collect::<Vec<_>>();
            let ptr = load(&arena, &bytes);
            let len = len as u32;
            {
                let mut state = arena.lock();
                let buffer = backing(&mut state, 0).as_ptr();
                let view = live_range(&mut state, ptr, len).unwrap();
                assert_eq!(view, bytes);
                assert_eq!(view.as_ptr(), buffer, "the input is decoded in place, not copied");
            }
            assert_eq!(arena.read(ptr + 1, len - 1).unwrap(), bytes[1..]);
            assert!(arena.read(ptr, len + 1).is_none());
            assert!(arena.read(0, len).is_none());
            assert!(arena.free(ptr, len));
            assert!(arena.read(ptr, len).is_none());
        }
    }

    #[test]
    fn small_allocations_stay_static_and_larger_ones_take_the_heap() {
        let arena = GuestArena::<{ 2 * STATIC_BYTES + 1 }>::new();
        let kinds = || {
            let state = arena.lock();
            state.live.each_ref().map(|allocation| match allocation {
                Allocation::Free => 'f',
                Allocation::Static(_) => 's',
                Allocation::Heap(_) => 'h',
            })
        };
        let whole = arena.allocate(STATIC_BYTES as u32);
        assert_eq!(kinds(), ['s', 'f']);
        assert!(arena.free(whole, STATIC_BYTES as u32));
        let first = arena.allocate(STATIC_BYTES as u32 - 1);
        // The second allocation no longer fits the remaining static byte.
        let second = arena.allocate(2);
        assert_eq!(kinds(), ['s', 'h']);
        assert_eq!(second, first + STATIC_BYTES as u32 - 1);
        assert!(arena.free(second, 2));
        assert!(arena.free(first, STATIC_BYTES as u32 - 1));
        let large = arena.allocate(STATIC_BYTES as u32 + 1);
        let small = arena.allocate(STATIC_BYTES as u32);
        assert_eq!(kinds(), ['h', 's']);
        assert_eq!(arena.allocate(1), 0);
        assert!(arena.free(small, STATIC_BYTES as u32));
        assert!(arena.free(large, STATIC_BYTES as u32 + 1));
        assert_eq!(arena.live(), (0, 0));
    }

    #[test]
    fn dispatch_preserves_live_input_and_lifo_output_ownership() {
        let bytes = input();
        let arena = GuestArena::<1024>::new();
        let ptr = load(&arena, &bytes);
        let len = bytes.len() as u32;
        let packed = dispatch_message::<Echo, 1024>(&arena, ptr, len);
        let output_ptr = (packed >> 32) as u32;
        let output_len = packed as u32;
        assert_eq!(output_ptr, ptr + len);
        assert_eq!(
            arena.read(output_ptr, output_len).unwrap(),
            crate::encode_message_output(&Echo::process_message(
                decode_message_input(&bytes).unwrap()
            ))
            .unwrap(),
        );
        assert_eq!(arena.read(ptr, len).unwrap(), bytes);
        assert_eq!(arena.allocate(1), 0);
        assert!(!arena.free(ptr, len));
        assert!(!arena.free(output_ptr, output_len - 1));
        assert!(arena.free(output_ptr, output_len));
        assert!(arena.free(ptr, len));
        assert_eq!(arena.allocate(len), ptr);
    }

    #[test]
    fn dispatch_failure_capacity_and_overflow_leave_input_owned() {
        let bytes = input();
        let arena = GuestArena::<480>::new();
        let ptr = load(&arena, &bytes);
        let len = bytes.len() as u32;
        assert_eq!(arena.allocate(0), 0);
        assert_eq!(arena.allocate(u32::MAX), 0);
        assert_eq!(dispatch_message::<Echo, 480>(&arena, ptr, len), 0);
        assert_eq!(arena.live().0, 1);
        assert_eq!(arena.read(ptr, len).unwrap(), bytes);
        assert_eq!(dispatch_init::<Echo, 480>(&arena, ptr, len), 0);
        assert_eq!(dispatch_message::<Echo, 480>(&arena, ptr, len - 1), 0);
        assert_eq!(arena.live().1, len as usize);
        assert!(arena.free(ptr, len));
        assert!(!arena.free(ptr, len));
    }

    #[test]
    fn direct_dispatch_writes_once_without_the_vec_growth_path() {
        let bytes = input();
        let expected = crate::encode_message_output(&Echo::process_message(
            decode_message_input(&bytes).unwrap(),
        ))
        .unwrap();
        let (vec_bytes, growths, _) = crate::output::work::take();
        assert_eq!(vec_bytes, expected.len());
        assert!(growths > 1, "the public Vec serializer is a live positive control");
        let arena = GuestArena::<1024>::new();
        let ptr = load(&arena, &bytes);
        let packed = dispatch_message::<Echo, 1024>(&arena, ptr, bytes.len() as u32);
        assert_eq!(crate::output::work::take(), (0, 0, expected.len()));
        assert_eq!(arena.read((packed >> 32) as u32, packed as u32).unwrap(), expected);
        assert_eq!(arena.live().0, 2);
    }

    #[test]
    fn ten_thousand_wallet_sized_outputs_remove_observed_vec_growth_and_copy() {
        // A work-count fixture, not a replacement for the registered Wallet benchmark.
        struct OutputShape;
        impl Contract for OutputShape {
            fn init(input: MessageInput<'_>) -> MessageOutput {
                Self::process_message(input)
            }
            fn process_message(input: MessageInput<'_>) -> MessageOutput {
                let mut output = MessageOutput::success(StateUpdate::FullReplace(vec![1; 144]));
                output.outbound_intents.push(crate::OutboundIntent {
                    input_index: 0,
                    destination: input.destination,
                    fuel_fee_shells: 1,
                    extra: Vec::new(),
                    method_id: 1,
                    body: vec![2; 64],
                });
                output
            }
        }
        let bytes = input();
        crate::output::work::take();
        for _ in 0..10_000 {
            let output = OutputShape::process_message(decode_message_input(&bytes).unwrap());
            let expected = crate::encode_message_output(&output).unwrap();
            assert_eq!(expected.len(), 339);
            let arena = GuestArena::<1024>::new();
            let ptr = load(&arena, &bytes);
            let packed = dispatch_message::<OutputShape, 1024>(&arena, ptr, bytes.len() as u32);
            assert_eq!(arena.read((packed >> 32) as u32, packed as u32).unwrap(), expected);
        }
        // Each positive-control Vec grows six times (first allocation included).
        // The direct dispatch adds no Vec writes/growth; only one arena write/byte.
        assert_eq!(crate::output::work::take(), (3_390_000, 60_000, 3_390_000));
    }

    #[test]
    fn batch_dispatch_preserves_count_mode_and_failed_index_checks() {
        struct BatchEcho;
        impl Contract for BatchEcho {
            fn init(input: MessageInput<'_>) -> MessageOutput {
                Echo::init(input)
            }
            fn process_message(input: MessageInput<'_>) -> MessageOutput {
                Echo::process_message(input)
            }
        }
        impl BatchContract for BatchEcho {
            fn process_batch(input: crate::BatchInput<'_>) -> crate::BatchOutput {
                crate::BatchOutput {
                    state_update: StateUpdate::FullReplace(input.state.to_vec()),
                    status_codes: vec![0, 1000],
                    outbound_intents: Vec::new(),
                    event_intents: Vec::new(),
                }
            }
        }
        let message = input();
        let mut bytes = vec![0; 158];
        bytes[..4].copy_from_slice(b"ACBI");
        bytes[52..116].fill(2);
        bytes[116..148].fill(3);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        for offset in [0u32, 176, 352] {
            bytes.extend_from_slice(&offset.to_le_bytes());
        }
        bytes.extend_from_slice(&message[273..449]);
        bytes.extend_from_slice(&message[273..449]);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        let input = decode_batch_input(&bytes).unwrap();
        let mut output = BatchEcho::process_batch(input);
        let expected = crate::encode_batch_output(&output).unwrap();
        assert_eq!(batch_output_len_for(&input, &output).unwrap(), expected.len());
        let arena = GuestArena::<1024>::new();
        let ptr = load(&arena, &bytes);
        let packed = dispatch_batch::<BatchEcho, 1024>(&arena, ptr, bytes.len() as u32);
        assert_eq!(arena.read((packed >> 32) as u32, packed as u32).unwrap(), expected);
        assert_eq!(arena.read(ptr, bytes.len() as u32).unwrap(), bytes);
        output.status_codes.pop();
        output.state_update = StateUpdate::DataPatchList(Vec::new());
        assert_eq!(batch_output_len_for(&input, &output), Err(EncodeError::InvalidIndex));
        output.status_codes.push(1000);
        assert_eq!(batch_output_len_for(&input, &output), Err(EncodeError::StateModeMismatch));
        output.outbound_intents.push(crate::OutboundIntent {
            input_index: 1,
            destination: input.destination,
            fuel_fee_shells: 0,
            extra: Vec::new(),
            method_id: 1,
            body: Vec::new(),
        });
        assert_eq!(batch_output_len_for(&input, &output), Err(EncodeError::InvalidIndex));
    }

    #[test]
    fn bounded_writer_failures_release_only_the_output_reservation() {
        let arena = GuestArena::<16>::new();
        let ptr = load(&arena, b"live");
        for length in [0, 13, usize::MAX] {
            assert_eq!(arena.write_output(length, |_| panic!("must reject before writing")), 0);
        }
        assert_eq!(arena.write_output(4, |writer| writer.write(b"short")), 0);
        assert_eq!(arena.write_output(4, |writer| writer.write(b"x")), 0);
        assert_eq!(arena.write_output(4, |_| Err(EncodeError::InvalidEvent)), 0);
        assert_eq!(arena.live(), (1, 4));
        assert_eq!(arena.read(ptr, 4).unwrap(), b"live");
        let packed = arena.write_output(12, |writer| writer.write(b"exactly fits"));
        assert_ne!(packed, 0);
        assert_eq!(arena.read((packed >> 32) as u32, packed as u32).unwrap(), b"exactly fits");
        assert!(arena.free((packed >> 32) as u32, packed as u32));
        assert!(arena.free(ptr, 4));
    }

    #[test]
    fn direct_message_sections_and_batch_match_the_public_serializer() {
        use crate::{
            BatchOutput, CallbackIntent, CanonicalMessage, CodeUpgradeIntent, CurrencyAmount,
            EventIntent, EventKind, OutboundIntent, StateOutputMode, StatePatch,
        };
        let bytes = input();
        let mut input = decode_message_input(&bytes).unwrap();
        let CanonicalMessage::Internal(ref mut message) = input.message else { panic!() };
        message.flags |= 4;
        message.method_id = crate::CODE_UPGRADE_METHOD_ID;
        for state in [
            StateUpdate::FullReplace(vec![5; 144]),
            StateUpdate::DataPatchList(vec![StatePatch {
                offset: 1,
                old_len: 1,
                new_bytes: vec![6; 4096],
            }]),
            StateUpdate::DataPatchList(Vec::new()),
        ] {
            input.required_state_output_mode = state.mode();
            let mut output = MessageOutput::success(state.clone());
            output.callback_result_body = vec![7; 19];
            output.outbound_intents.push(OutboundIntent {
                input_index: 0,
                destination: input.destination,
                fuel_fee_shells: 9,
                extra: vec![
                    CurrencyAmount { currency_id: 0, amount: 8 },
                    CurrencyAmount { currency_id: 1, amount: 10 },
                ],
                method_id: 11,
                body: vec![12; 64],
            });
            output
                .callback_intents
                .push(CallbackIntent { outbound_index: 0, handler_method_id: 13 });
            output.event_intents.push(EventIntent {
                input_index: 0,
                kind: EventKind::Internal,
                event_class: 14,
                event_name: "changed".into(),
                topics: vec![vec![15; 17], Vec::new()],
                fields_binary: crate::encode_event_fields(&[]).unwrap(),
            });
            output.code_upgrade_intent = Some(CodeUpgradeIntent {
                target_selected_code_id: [16; 32],
                expected_code_hash: [17; 32],
            });
            let expected = crate::encode_message_output(&output).unwrap();
            let len = message_output_len_for(&input, false, &output).unwrap();
            assert_eq!(len, expected.len());
            let arena = GuestArena::<8192>::new();
            let live = load(&arena, &bytes);
            let packed = arena.write_output(len, |writer| write_message_output(writer, &output));
            assert_eq!(arena.read((packed >> 32) as u32, packed as u32).unwrap(), expected);
            assert_eq!(arena.read(live, bytes.len() as u32).unwrap(), bytes);

            let batch = BatchOutput {
                state_update: state,
                status_codes: vec![0, 1000],
                outbound_intents: output.outbound_intents,
                event_intents: output.event_intents,
            };
            let expected = crate::encode_batch_output(&batch).unwrap();
            let arena = GuestArena::<8192>::new();
            let packed =
                arena.write_output(expected.len(), |writer| write_batch_output(writer, &batch));
            assert_eq!(arena.read((packed >> 32) as u32, packed as u32).unwrap(), expected);
        }
        let mut invalid = MessageOutput::failure(7);
        invalid.callback_result_body.push(1);
        assert_eq!(
            message_output_len_for(&input, true, &invalid),
            Err(EncodeError::InvalidCallback)
        );
        assert_eq!(
            message_output_len_for(&input, false, &invalid),
            Err(EncodeError::InvalidStatusCode)
        );
        input.required_state_output_mode = StateOutputMode::DataPatchList;
        let mut output = MessageOutput::success(StateUpdate::FullReplace(Vec::new()));
        assert_eq!(
            message_output_len_for(&input, false, &output),
            Err(EncodeError::UpgradeRequired)
        );
        output.code_upgrade_intent = Some(CodeUpgradeIntent {
            target_selected_code_id: [1; 32],
            expected_code_hash: [2; 32],
        });
        assert_eq!(
            message_output_len_for(&input, false, &output),
            Err(EncodeError::StateModeMismatch)
        );
    }
}
