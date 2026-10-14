use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use crate::input::{decode_batch_input, decode_message_input};
use crate::output::{
    batch_output_len_for, message_output_len_for, write_batch_output, write_message_output,
    EncodeError, OutputWriter,
};
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
        let mut chunks = source.chunks_exact(8);
        let mut words = Vec::with_capacity(length.div_ceil(8));
        words.extend(chunks.by_ref().map(|chunk| {
            [
                chunk[0].load(Ordering::Relaxed),
                chunk[1].load(Ordering::Relaxed),
                chunk[2].load(Ordering::Relaxed),
                chunk[3].load(Ordering::Relaxed),
                chunk[4].load(Ordering::Relaxed),
                chunk[5].load(Ordering::Relaxed),
                chunk[6].load(Ordering::Relaxed),
                chunk[7].load(Ordering::Relaxed),
            ]
        }));
        let mut bytes = words.into_flattened();
        bytes.extend(chunks.remainder().iter().map(|byte| byte.load(Ordering::Relaxed)));
        Some(bytes)
    }

    fn write_output(
        &self,
        length: usize,
        encode: impl FnOnce(&mut ArenaWriter<'_>) -> Result<(), EncodeError>,
    ) -> u64 {
        let Ok(len) = u32::try_from(length) else {
            return 0;
        };
        let ptr = self.allocate(len);
        if ptr == 0 {
            return 0;
        }
        let Some(start) = self.offset(ptr).and_then(|offset| usize::try_from(offset).ok()) else {
            return 0;
        };
        let Some(destination) = self.bytes.get(start..start.saturating_add(length)) else {
            self.free(ptr, len);
            return 0;
        };
        let mut writer = ArenaWriter { remaining: destination };
        if encode(&mut writer).is_err() || !writer.remaining.is_empty() {
            self.free(ptr, len);
            return 0;
        }
        (u64::from(ptr) << 32) | u64::from(len)
    }

    fn base_pointer(&self) -> Option<u32> {
        // Native unit tests use logical WASM addresses, never dereference them.
        #[cfg(test)]
        return Some(1);
        #[cfg(not(test))]
        u32::try_from(self.bytes.as_ptr() as usize).ok()
    }

    fn offset(&self, ptr: u32) -> Option<u32> {
        ptr.checked_sub(self.base_pointer()?)
            .filter(|offset| usize::try_from(*offset).ok().is_some_and(|offset| offset < N))
    }
}

struct ArenaWriter<'a> {
    remaining: &'a [AtomicU8],
}

impl OutputWriter for ArenaWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), EncodeError> {
        let (destination, remaining) =
            self.remaining.split_at_checked(bytes.len()).ok_or(EncodeError::LengthOverflow)?;
        for (destination, source) in destination.iter().zip(bytes) {
            destination.store(*source, Ordering::Relaxed);
        }
        self.remaining = remaining;
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
    let Ok(output_len) = message_output_len_for(&input, is_init, &output) else {
        return 0;
    };
    arena.write_output(output_len, |writer| write_message_output(writer, &output))
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
    let Ok(output_len) = batch_output_len_for(&input, &output) else {
        return 0;
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
        let mut bytes = vec![0; 278];
        bytes[..4].copy_from_slice(b"ACMI");
        bytes[60..124].fill(1);
        bytes[156..220].fill(2);
        bytes[220..252].fill(3);
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(b"old");
        bytes.extend_from_slice(&192u32.to_le_bytes());
        let mut message = vec![0; 192];
        message[8..16].copy_from_slice(&1u64.to_le_bytes());
        message[64..128].fill(1);
        message[128..192].fill(2);
        bytes.extend_from_slice(&message);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        decode_message_input(&bytes).unwrap();
        bytes
    }

    fn load<const N: usize>(arena: &GuestArena<N>, bytes: &[u8]) -> u32 {
        let ptr = arena.allocate(bytes.len() as u32);
        assert_ne!(ptr, 0);
        for (dst, byte) in arena.bytes.iter().zip(bytes) {
            dst.store(*byte, Ordering::Relaxed);
        }
        ptr
    }

    #[test]
    fn input_copy_preserves_bytes_and_live_bounds_at_chunk_boundaries() {
        let arena = GuestArena::<66_016>::new();
        for len in [1, 7, 8, 9, 15, 16, 17, 65_536, 66_006] {
            let bytes = (0..len).map(|index| (index % 251) as u8).collect::<Vec<_>>();
            let ptr = load(&arena, &bytes);
            let len = len as u32;
            let copied = arena.read(ptr, len).unwrap();
            assert_eq!(copied, bytes);
            assert_eq!(copied.capacity(), (len as usize).div_ceil(8) * 8);
            assert_eq!(arena.read(ptr + 1, len - 1).unwrap(), bytes[1..]);
            assert!(arena.read(ptr, len + 1).is_none());
            assert!(arena.read(0, len).is_none());
            assert!(arena.free(ptr, len));
            assert!(arena.read(ptr, len).is_none());
        }
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
        let arena = GuestArena::<512>::new();
        let ptr = load(&arena, &bytes);
        let len = bytes.len() as u32;
        assert_eq!(arena.allocate(0), 0);
        assert_eq!(arena.allocate(u32::MAX), 0);
        assert_eq!(dispatch_message::<Echo, 512>(&arena, ptr, len), 0);
        assert_eq!(arena.allocation_count.load(Ordering::Relaxed), 1);
        assert_eq!(arena.read(ptr, len).unwrap(), bytes);
        assert_eq!(dispatch_init::<Echo, 512>(&arena, ptr, len), 0);
        assert_eq!(dispatch_message::<Echo, 512>(&arena, ptr, len - 1), 0);
        assert_eq!(arena.next.load(Ordering::Relaxed), len);
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
        assert_eq!(arena.allocation_count.load(Ordering::Relaxed), 2);
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
                    value_main: 0,
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
            assert_eq!(expected.len(), 355);
            let arena = GuestArena::<1024>::new();
            let ptr = load(&arena, &bytes);
            let packed = dispatch_message::<OutputShape, 1024>(&arena, ptr, bytes.len() as u32);
            assert_eq!(arena.read((packed >> 32) as u32, packed as u32).unwrap(), expected);
        }
        // Each positive-control Vec grows six times (first allocation included).
        // The direct dispatch adds no Vec writes/growth; only one arena write/byte.
        assert_eq!(crate::output::work::take(), (3_550_000, 60_000, 3_550_000));
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
        let mut bytes = vec![0; 174];
        bytes[..4].copy_from_slice(b"ACBI");
        bytes[52..116].fill(2);
        bytes[116..148].fill(3);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        for offset in [0u32, 192, 384] {
            bytes.extend_from_slice(&offset.to_le_bytes());
        }
        bytes.extend_from_slice(&message[289..481]);
        bytes.extend_from_slice(&message[289..481]);
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
            value_main: 0,
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
        assert_eq!(arena.allocation_count.load(Ordering::Relaxed), 1);
        assert_eq!(arena.next.load(Ordering::Relaxed), 4);
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
                value_main: 8,
                fuel_fee_shells: 9,
                extra: vec![CurrencyAmount { currency_id: 1, amount: 10 }],
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
