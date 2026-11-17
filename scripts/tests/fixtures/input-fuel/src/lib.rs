use avm_contract_sdk::{
    set_owner_key, Contract, MessageInput, MessageOutput, StateOutputMode, StatePatch,
    StateUpdate, OWNER_KEY_BYTES,
};

// Account data: the owner key at `owner_key_offset = 0`, then appended bytes.
const OWNER_KEY_OFFSET: u32 = 0;
const FILL: u8 = 0xa5;
const PROBE: u32 = 0x8000_0000;

struct InputFuel;

impl Contract for InputFuel {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        let mut data = vec![0; OWNER_KEY_BYTES];
        if let Ok(deploy) = input.deploy() {
            // The key slot is inside the data built above.
            let _ = set_owner_key(&mut data, OWNER_KEY_OFFSET, deploy.owner_id());
        }
        MessageOutput::success(StateUpdate::FullReplace(data))
    }

    /// `n` appends `n` fill bytes to the data. `PROBE | n` checks that the
    /// input holds the whole `n`-byte data, reading only its first and last
    /// bytes, and changes nothing: its cost does not depend on the data.
    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        let Ok(bytes) = <[u8; 4]>::try_from(input.message.body()) else {
            return MessageOutput::failure(1000);
        };
        let argument = u32::from_le_bytes(bytes);
        let full = input.required_state_output_mode == StateOutputMode::FullReplace;
        if argument & PROBE != 0 {
            let state = input.state;
            if state.len() != (argument & !PROBE) as usize
                || state.len() <= OWNER_KEY_BYTES
                || state[..OWNER_KEY_BYTES].iter().all(|byte| *byte == 0)
                || state.last() != Some(&FILL)
            {
                return MessageOutput::failure(1001);
            }
            if full {
                return MessageOutput::failure(1002);
            }
            return MessageOutput::success(StateUpdate::DataPatchList(Vec::new()));
        }
        // `Vec::resize` with a run-time value, as contracts commonly extend data.
        let fill = core::hint::black_box(FILL);
        let length = argument as usize;
        if full {
            let mut data = input.state.to_vec();
            data.resize(data.len() + length, fill);
            return MessageOutput::success(StateUpdate::FullReplace(data));
        }
        let mut appended = Vec::new();
        appended.resize(length, fill);
        MessageOutput::success(StateUpdate::DataPatchList(vec![StatePatch {
            offset: input.state.len() as u32,
            old_len: 0,
            new_bytes: appended,
        }]))
    }
}

avm_contract_sdk::export_message_contract!(InputFuel, arena_bytes = 1_572_864);
