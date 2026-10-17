//! Local test fixture only: never a production contract or financial component.
#![forbid(unsafe_code)]
use avm_contract_sdk::{
    Contract, MessageInput, MessageOutput, StateOutputMode, StatePatch, StateUpdate,
};
use avm_revenue_core::deploy_data;
struct Boundary;
impl Contract for Boundary {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        let Ok(data) = deploy_data(input.message.body()) else {
            return MessageOutput::failure(3900);
        };
        let Ok(bytes) = <[u8; 4]>::try_from(data) else {
            return MessageOutput::failure(3900);
        };
        let len = u32::from_le_bytes(bytes) as usize;
        if input.required_state_output_mode != StateOutputMode::FullReplace
            || !(5..=70_000).contains(&len)
        {
            return MessageOutput::failure(3900);
        }
        let mut state = vec![0; len];
        state[..4].copy_from_slice(&((len - 4) as u32).to_le_bytes());
        MessageOutput::success(StateUpdate::FullReplace(state))
    }
    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        // 0 follows the host, 1 forces FullReplace, 2 forces DataPatchList.
        // Deliberately wrong modes exercise the actual guest ABI validation.
        let mode = match input.message.body() {
            [0] => input.required_state_output_mode,
            [1] => StateOutputMode::FullReplace,
            [2] => StateOutputMode::DataPatchList,
            _ => return MessageOutput::failure(3900),
        };
        let value = input.required_state_output_mode as u8;
        let update = match mode {
            StateOutputMode::FullReplace => {
                let mut state = input.state.to_vec();
                state[4] = value;
                StateUpdate::FullReplace(state)
            }
            StateOutputMode::DataPatchList => StateUpdate::DataPatchList(vec![StatePatch {
                offset: 4,
                old_len: 1,
                new_bytes: vec![value],
            }]),
        };
        MessageOutput::success(update)
    }
}
avm_contract_sdk::export_message_contract!(Boundary, arena_bytes = 524_288);
