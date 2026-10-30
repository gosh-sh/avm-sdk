#![forbid(unsafe_code)]

use avm_contract_sdk::{MessageInput, MessageOutput, OutboundIntent, StateUpdate};

// Build regenerates this file before each clean compilation. No copied IDs.
pub mod generated {
    include!(env!("AVM_CONTRACT_BINDINGS"));
}
use generated::{errors, methods, types, DispatchError, External, Internal};

struct Handlers;

impl generated::Handlers for Handlers {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        let Ok(deploy) = input.deploy() else {
            return MessageOutput::failure(errors::INVALID_BODY);
        };
        match generated::decode_type2(deploy.init_data()) {
            Ok(bytes) => MessageOutput::success(StateUpdate::FullReplace(bytes)),
            Err(_) => MessageOutput::failure(errors::INVALID_BODY),
        }
    }

    fn on_dispatch_error(_: MessageInput<'_>, error: DispatchError) -> MessageOutput {
        MessageOutput::failure(match error {
            DispatchError::MalformedBody(_) => errors::INVALID_BODY,
            DispatchError::WrongCaller => errors::WRONG_CALLER,
            DispatchError::UnknownMethod => errors::UNKNOWN_METHOD,
        })
    }

    fn tools_add(ctx: External<'_>, args: types::Type4) -> MessageOutput {
        let mut body = args.a.to_le_bytes().to_vec();
        body.extend_from_slice(&args.b.to_le_bytes());
        let mut output = unchanged(ctx.input);
        output.outbound_intents.push(outbound(ctx.input, methods::TOOLS_SUM, body));
        output
    }

    fn tools_sum(ctx: Internal<'_>, args: types::Type4) -> MessageOutput {
        if ctx.sender != ctx.input.destination {
            return MessageOutput::failure(errors::WRONG_CALLER);
        }
        match args.a.checked_add(args.b) {
            Some(value) => {
                MessageOutput::success(StateUpdate::FullReplace(value.to_le_bytes().to_vec()))
            }
            None => MessageOutput::failure(errors::OVERFLOW),
        }
    }
}

fn unchanged(input: MessageInput<'_>) -> MessageOutput {
    MessageOutput::success(StateUpdate::FullReplace(input.state.to_vec()))
}

fn outbound(input: MessageInput<'_>, method_id: u64, body: Vec<u8>) -> OutboundIntent {
    OutboundIntent {
        input_index: 0,
        destination: input.destination,
        fuel_fee_shells: 1000,
        extra: Vec::new(),
        method_id,
        body,
    }
}

avm_contract_sdk::export_message_contract!(generated::Contract<Handlers>, arena_bytes = 262_144);
