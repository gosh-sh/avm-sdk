#![forbid(unsafe_code)]
use avm_contract_sdk::{
    encode_event_fields, CallbackIntent, Caller, CanonicalMessage, Contract, CurrencyAmount,
    CurrencyAmounts, EventField, EventIntent, EventKind, EventValue, MessageInput, MessageOutput,
    OutboundIntent, StateUpdate,
};
use avm_stream_core::{state::*, *};
pub struct Vesting;
impl Contract for Vesting {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        initialize(input).unwrap_or_else(MessageOutput::failure)
    }
    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        process(input).unwrap_or_else(MessageOutput::failure)
    }
}
fn zero_value(input: &MessageInput<'_>) -> ResultCode<()> {
    if !input.message.extra().is_empty() {
        Err(E_VALUE)
    } else {
        Ok(())
    }
}
fn initialize(input: MessageInput<'_>) -> ResultCode<MessageOutput> {
    let config = Config::decode(deploy_data(input)?)?;
    if input.caller != Caller::External(config.owner) {
        return Err(E_AUTH);
    }
    zero_value(&input)?;
    if !input.state.is_empty()
        || input.context.block_time >= config.first_start()
        || config.streams[..usize::from(config.count)]
            .iter()
            .flat_map(|s| [s.payout, s.refund])
            .any(|a| a == input.destination || a.dapp_id != input.destination.dapp_id)
    {
        return Err(E_ARGS);
    }
    finish(
        State::new(config, input.context.block_time),
        ids::EVENT_CREATED,
        "stream.created",
        255,
        255,
        0,
    )
}
fn process(input: MessageInput<'_>) -> ResultCode<MessageOutput> {
    let mut s = State::decode(input.state)?;
    s.last_time = s.last_time.max(input.context.block_time);
    if input.message.is_callback() {
        return callback(input, s);
    }
    let Caller::External(actor) = input.caller else {
        return Err(E_AUTH);
    };
    let method = input.message.method_id();
    let body = input.message.body();
    if method == ids::TOPUP {
        no_body(body)?;
        if input.message.extra().is_empty()
            || input
                .message
                .extra()
                .iter()
                .any(|c| c.currency_id != SHELL_CURRENCY_ID || c.amount == 0)
        {
            return Err(E_VALUE);
        }
        return finish(s, ids::EVENT_FEES, "stream.fees", 255, 255, 0);
    }
    if method == ids::FUND {
        no_body(body)?;
        if actor != s.config.owner {
            return Err(E_AUTH);
        }
        if s.funded || s.last_time >= s.config.first_start() {
            return Err(E_PHASE);
        }
        let total = s.config.total()?;
        if principal_value(input.message.extra()) != Some(total) {
            return Err(E_VALUE);
        }
        if principal_balance(input.extra) < total {
            return Err(E_BALANCE);
        }
        s.funded = true;
        return finish(s, ids::EVENT_FUNDED, "stream.funded", 255, 255, total);
    }
    zero_value(&input)?;
    if method == ids::STATUS {
        no_body(body)?;
        return finish(s, ids::EVENT_STATUS, "stream.status", 255, 255, 0);
    }
    if body.len() != 1 || body[0] >= s.config.count {
        return Err(E_ARGS);
    }
    let i = usize::from(body[0]);
    let cfg = s.config.streams[i];
    if !s.funded {
        return Err(E_PHASE);
    }
    if method == ids::CANCEL {
        if actor != s.config.owner {
            return Err(E_AUTH);
        }
        if !cfg.revocable {
            return Err(E_IRREVOCABLE);
        }
        if s.progress[i].cancelled || s.last_time >= cfg.end {
            return Err(E_PHASE);
        }
        let cap = cfg.earned(s.last_time)?;
        s.progress[i].cancelled = true;
        s.progress[i].cutoff = s.last_time;
        s.progress[i].cap = cap;
        return finish(s, ids::EVENT_CANCELLED, "stream.cancelled", i as u8, 255, cap);
    }
    let (kind, destination) = if method == ids::CLAIM {
        if actor != cfg.key {
            return Err(E_AUTH);
        }
        (0, cfg.payout)
    } else if method == ids::REFUND {
        if actor != s.config.owner {
            return Err(E_AUTH);
        }
        (1, cfg.refund)
    } else {
        return Err(E_ARGS);
    };
    if s.progress[i].lanes[kind].pending != 0 {
        return Err(E_BUSY);
    }
    let amount = s.due(i, kind, s.last_time)?;
    if amount == 0 {
        return Err(E_EMPTY);
    }
    if principal_balance(input.extra) < s.held()?
        || !input
            .extra
            .iter()
            .any(|c| c.currency_id == SHELL_CURRENCY_ID && c.amount >= s.config.fuel)
    {
        return Err(E_BALANCE);
    }
    let nonce = s.nonce.checked_add(1).ok_or(E_OVERFLOW)?;
    s.nonce = nonce;
    let lane = &mut s.progress[i].lanes[kind];
    lane.pending = amount;
    lane.nonce = nonce;
    lane.after_lt = input.context.block_lt.max(lane.last_lt);
    let fuel = s.config.fuel;
    let mut out = finish(s, ids::EVENT_PENDING, "stream.pending", i as u8, kind as u8, amount)?;
    out.outbound_intents.push(OutboundIntent {
        input_index: 0,
        destination,
        fuel_fee_shells: fuel,
        extra: vec![CurrencyAmount { currency_id: PRINCIPAL_CURRENCY_ID, amount }],
        method_id: ids::RECEIVE,
        body: payment_body(i as u8, kind as u8, nonce, amount),
    });
    out.callback_intents
        .push(CallbackIntent { outbound_index: 0, handler_method_id: ids::CALLBACK });
    Ok(out)
}
fn callback(input: MessageInput<'_>, mut s: State) -> ResultCode<MessageOutput> {
    let (Caller::Internal(source), CanonicalMessage::Internal(msg)) = (input.caller, input.message)
    else {
        return Err(E_CALLBACK);
    };
    if msg.method_id != ids::CALLBACK || !msg.is_callback() || msg.has_callback_request() {
        return Err(E_CALLBACK);
    }
    let header = msg.callback.ok_or(E_CALLBACK)?;
    if header.cause_address != input.destination || header.cause_msg_hash == [0; 32] {
        return Err(E_CALLBACK);
    }
    let (i, kind) = s.config.streams[..usize::from(s.config.count)]
        .iter()
        .enumerate()
        .find_map(|(i, c)| {
            if c.payout == source {
                Some((i, 0))
            } else if c.refund == source {
                Some((i, 1))
            } else {
                None
            }
        })
        .ok_or(E_CALLBACK)?;
    let lane = s.progress[i].lanes[kind];
    if lane.pending == 0
        || msg.created_lt <= lane.after_lt
        || msg.created_lt <= lane.last_lt
        || header.cause_msg_hash == lane.last_hash
    {
        return Err(E_CALLBACK);
    }
    if header.status_code == 0 {
        if !callback_value(msg.extra, None)
            || read_payment(msg.body).map_err(|_| E_CALLBACK)?
                != (i, kind, lane.nonce, lane.pending)
        {
            return Err(E_CALLBACK);
        }
        let p = &mut s.progress[i];
        if kind == 0 {
            p.claimed = p.claimed.checked_add(lane.pending).ok_or(E_OVERFLOW)?
        } else {
            p.refunded = p.refunded.checked_add(lane.pending).ok_or(E_OVERFLOW)?
        }
    } else if !callback_value(msg.extra, Some(lane.pending)) || !msg.body.is_empty() {
        return Err(E_CALLBACK);
    }
    s.progress[i].lanes[kind] =
        Lane { last_lt: msg.created_lt, last_hash: header.cause_msg_hash, ..Lane::default() };
    let (id, name) = if header.status_code == 0 {
        (ids::EVENT_SETTLED, "stream.settled")
    } else {
        (ids::EVENT_FAILED, "stream.failed")
    };
    finish(s, id, name, i as u8, kind as u8, lane.pending)
}

// Native callbacks may return unused FUEL as SHELL, separately from principal.
// Canonical decoding has already established sorted, unique ECC ids.
fn callback_value(extra: CurrencyAmounts<'_>, principal: Option<u128>) -> bool {
    let mut has_principal = principal.is_none();
    for currency in extra {
        match currency.currency_id {
            PRINCIPAL_CURRENCY_ID if principal == Some(currency.amount) => has_principal = true,
            SHELL_CURRENCY_ID if currency.amount > 0 => {}
            _ => return false,
        }
    }
    has_principal
}
fn finish(
    s: State,
    class: u64,
    name: &str,
    index: u8,
    kind: u8,
    amount: u128,
) -> ResultCode<MessageOutput> {
    s.validate()?;
    let (claimed, refunded, pending) = s.totals()?;
    let fields = vec![
        EventField { name: "amount".into(), value: EventValue::U128(amount) },
        EventField { name: "claimed".into(), value: EventValue::U128(claimed) },
        EventField { name: "held".into(), value: EventValue::U128(s.held()?) },
        EventField { name: "index".into(), value: EventValue::U64(u64::from(index)) },
        EventField { name: "kind".into(), value: EventValue::U64(u64::from(kind)) },
        EventField { name: "nonce".into(), value: EventValue::U64(s.nonce) },
        EventField { name: "pending".into(), value: EventValue::U128(pending) },
        EventField { name: "refunded".into(), value: EventValue::U128(refunded) },
        EventField { name: "time".into(), value: EventValue::U64(u64::from(s.last_time)) },
    ];
    let mut out = MessageOutput::success(StateUpdate::FullReplace(s.encode()));
    out.event_intents.push(EventIntent {
        input_index: 0,
        kind: EventKind::External,
        event_class: class,
        event_name: name.into(),
        topics: vec![],
        fields_binary: encode_event_fields(&fields).map_err(|_| E_STATE)?,
    });
    Ok(out)
}
#[cfg(target_arch = "wasm32")]
avm_contract_sdk::export_message_contract!(Vesting, arena_bytes = 262_144);
