use avm_contract_sdk::{Contract, MessageInput, MessageOutput, StateUpdate};

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
    fn init(_: MessageInput<'_>) -> MessageOutput {
        MessageOutput::success(StateUpdate::FullReplace(0_u32.to_le_bytes().to_vec()))
    }

    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        let Ok(bytes) = <[u8; 4]>::try_from(input.message.body()) else {
            return MessageOutput::failure(1000);
        };
        let pages = u32::from_le_bytes(bytes);
        if pages & 0x8000_0000 != 0 {
            return MessageOutput::success(StateUpdate::FullReplace(
                use_stack(pages & 0x7fff_ffff).to_le_bytes().to_vec(),
            ));
        }
        let previous = core::arch::wasm32::memory_grow::<0>(pages as usize) as u32;
        MessageOutput::success(StateUpdate::FullReplace(previous.to_le_bytes().to_vec()))
    }
}

avm_contract_sdk::export_message_contract!(MemoryBudget, arena_bytes = 65_536);
