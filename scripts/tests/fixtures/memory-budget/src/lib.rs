use avm_contract_sdk::{
    set_owner_key, Contract, MessageInput, MessageOutput, StateUpdate, OWNER_KEY_BYTES,
};

// Account data: the owner key at `owner_key_offset = 0`, then the probe result.
const OWNER_KEY_OFFSET: u32 = 0;

struct MemoryBudget;

#[inline(never)]
fn use_stack(depth: u32) -> u64 {
    let frame = std::hint::black_box([u64::from(depth); 32]);
    if depth == 0 {
        frame[0]
    } else {
        use_stack(depth - 1) ^ std::hint::black_box(frame[(depth % 32) as usize])
    }
}

impl Contract for MemoryBudget {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        let mut data = vec![0; OWNER_KEY_BYTES + 4];
        if let Ok(deploy) = input.deploy() {
            // The key slot is inside the data built above.
            let _ = set_owner_key(&mut data, OWNER_KEY_OFFSET, deploy.owner_id());
        }
        MessageOutput::success(StateUpdate::FullReplace(data))
    }

    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        let Ok(bytes) = <[u8; 4]>::try_from(input.message.body()) else {
            return MessageOutput::failure(1000);
        };
        let Some(key) = input.state.get(..OWNER_KEY_BYTES) else {
            return MessageOutput::failure(1001);
        };
        let keyed = |value: &[u8]| StateUpdate::FullReplace([key, value].concat());
        let pages = u32::from_le_bytes(bytes);
        if pages & 0x8000_0000 != 0 {
            return MessageOutput::success(keyed(&use_stack(pages & 0x7fff_ffff).to_le_bytes()));
        }
        let previous = core::arch::wasm32::memory_grow::<0>(pages as usize) as u32;
        MessageOutput::success(keyed(&previous.to_le_bytes()))
    }
}

avm_contract_sdk::export_message_contract!(MemoryBudget, arena_bytes = 65_536);
