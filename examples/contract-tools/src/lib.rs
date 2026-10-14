#![forbid(unsafe_code)]

use avm_contract_sdk::{
    encode_event_fields, EventField, EventIntent, EventKind, EventValue, InternalEventReadIntent,
    InternalEventReadRequest, MessageInput, MessageOutput, OutboundIntent, StateUpdate,
};

// Build regenerates this file before each clean compilation. No copied IDs.
pub mod generated {
    include!(env!("AVM_CONTRACT_BINDINGS"));
}
use generated::{errors, methods, types, DispatchError, External, Internal};

const EVENT_CLASS: u64 = 0x7175;
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

    fn tools_cycle(ctx: External<'_>, emit_first: types::Type1) -> MessageOutput {
        let mut output = unchanged(ctx.input);
        if emit_first {
            output.outbound_intents.push(outbound(
                ctx.input,
                methods::TOOLS_EMIT,
                17u64.to_le_bytes().to_vec(),
            ));
        }
        output.internal_event_read_intents.push(InternalEventReadIntent {
            outbound_index: output.outbound_intents.len() as u32,
            requests: vec![InternalEventReadRequest {
                source: ctx.input.destination,
                event_class: EVENT_CLASS,
            }],
        });
        output.outbound_intents.push(outbound(ctx.input, methods::TOOLS_COLLECT, Vec::new()));
        output
    }

    fn tools_emit(ctx: Internal<'_>, value: types::Type3) -> MessageOutput {
        if ctx.sender != ctx.input.destination {
            return MessageOutput::failure(errors::WRONG_CALLER);
        }
        let Ok(fields_binary) = encode_event_fields(&[EventField {
            name: "value".into(),
            value: EventValue::U64(value),
        }]) else {
            return MessageOutput::failure(errors::INVALID_BODY);
        };
        let mut output = unchanged(ctx.input);
        output.event_intents.push(EventIntent {
            input_index: 0,
            kind: EventKind::Internal,
            event_class: EVENT_CLASS,
            event_name: "tools.changed".into(),
            topics: Vec::new(),
            fields_binary,
        });
        output
    }

    fn tools_collect(ctx: Internal<'_>, _: &[u8]) -> MessageOutput {
        if ctx.sender != ctx.input.destination {
            return MessageOutput::failure(errors::WRONG_CALLER);
        }
        // These are the bounded canonical binary results already resolved from
        // the recipient's block-start snapshot. No Cap'n Proto guest parser.
        MessageOutput::success(StateUpdate::FullReplace(ctx.input.internal_event_results.to_vec()))
    }
}

fn unchanged(input: MessageInput<'_>) -> MessageOutput {
    MessageOutput::success(StateUpdate::FullReplace(input.state.to_vec()))
}

fn outbound(input: MessageInput<'_>, method_id: u64, body: Vec<u8>) -> OutboundIntent {
    OutboundIntent {
        input_index: 0,
        destination: input.destination,
        value_main: 0,
        fuel_fee_shells: 1000,
        extra: Vec::new(),
        method_id,
        body,
    }
}

avm_contract_sdk::export_message_contract!(generated::Contract<Handlers>, arena_bytes = 262_144);
