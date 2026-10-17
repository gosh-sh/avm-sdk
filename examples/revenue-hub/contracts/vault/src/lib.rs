#![forbid(unsafe_code)]

use avm_contract_sdk::{
    CallbackIntent, Caller, CanonicalMessage, Contract, EventField, EventValue, MessageInput,
    MessageOutput, OutboundIntent, StateUpdate,
};
use avm_revenue_core::{vault::*, *};

pub struct RevenueVault;
impl Contract for RevenueVault {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        initialize(input).unwrap_or_else(MessageOutput::failure)
    }
    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        process(input).unwrap_or_else(MessageOutput::failure)
    }
}

fn initialize(input: MessageInput<'_>) -> HubResult<MessageOutput> {
    let config = Config::decode(deploy_data(input.message.body())?)?;
    let Caller::External(actor) = input.caller else {
        return Err(E_AUTH);
    };
    config.signer(actor)?;
    if !input.state.is_empty()
        || config.members[..usize::from(config.count)].iter().any(|m| {
            m.destination == input.destination || m.destination.dapp_id != input.destination.dapp_id
        })
    {
        return Err(E_ARGS);
    }
    zero_value(&input)?;
    finish(State::new(config), "hub.created", 255, 0)
}

fn process(input: MessageInput<'_>) -> HubResult<MessageOutput> {
    let mut s = State::decode(input.state)?;
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
        if input.message.value_main() != 0
            || input.message.extra().is_empty()
            || input.message.extra().iter().any(|c| c.currency_id != 2 || c.amount == 0)
        {
            return Err(E_VALUE);
        }
        return finish(s, "hub.fees_added", 255, 0);
    }
    if method == ids::DEPOSIT {
        no_body(body)?;
        if s.paused {
            return Err(E_PAUSED);
        }
        if input.message.extra().iter().any(|c| c.currency_id != 2 || c.amount == 0) {
            return Err(E_VALUE);
        }
        let amount = input.message.value_main();
        if amount == 0 {
            return Err(E_VALUE);
        }
        if s.held()?.checked_add(amount).is_none_or(|v| v > input.balance_main) {
            return Err(E_BALANCE);
        }
        s.deposit(amount)?;
        return finish(s, "hub.deposited", 255, amount);
    }
    zero_value(&input)?;
    match method {
        ids::STATUS => {
            no_body(body)?;
            finish(s, "hub.status", 255, 0)
        }
        ids::CLAIM => {
            if body.len() != 1 || body[0] >= s.config.count {
                return Err(E_ARGS);
            }
            let i = usize::from(body[0]);
            let member = s.config.members[i];
            if actor != member.key {
                return Err(E_AUTH);
            }
            if s.paused {
                return Err(E_PAUSED);
            }
            if s.members[i].pending != 0 {
                return Err(E_BUSY);
            }
            let amount = s.members[i].claimable;
            if amount == 0 {
                return Err(E_EMPTY);
            }
            if input.balance_main < s.held()?
                || !input.extra.iter().any(|c| c.currency_id == 2 && c.amount >= s.fuel_limit)
            {
                return Err(E_BALANCE);
            }
            let nonce = s.claim_nonce.checked_add(1).ok_or(E_OVERFLOW)?;
            let m = &mut s.members[i];
            m.pending = amount;
            m.claimable = 0;
            m.nonce = nonce;
            m.after_lt = input.context.block_lt.max(m.last_callback_lt);
            s.claim_nonce = nonce;
            let fuel = s.fuel_limit;
            let mut out = finish(s, "hub.claim_pending", i as u8, amount)?;
            out.outbound_intents.push(OutboundIntent {
                input_index: 0,
                destination: member.destination,
                value_main: amount,
                fuel_fee_shells: fuel,
                extra: vec![],
                method_id: ids::RECEIVE,
                body: payment_body(i as u8, nonce, amount),
            });
            out.callback_intents
                .push(CallbackIntent { outbound_index: 0, handler_method_id: ids::CALLBACK });
            Ok(out)
        }
        ids::PROPOSE => {
            let signer = s.config.signer(actor)?;
            let (nonce, action, value) = Proposal::decode_body(body)?;
            if s.proposal.nonce != 0 && input.context.block_time < s.proposal.expires {
                return Err(E_GOVERNANCE);
            }
            if nonce != s.proposal_nonce.checked_add(1).ok_or(E_OVERFLOW)? {
                return Err(E_GOVERNANCE);
            }
            let created = input.context.block_time;
            let ready = created.checked_add(s.config.delay).ok_or(E_OVERFLOW)?;
            let expires = ready.checked_add(s.config.vote_window).ok_or(E_OVERFLOW)?;
            s.proposal =
                Proposal { nonce, action, votes: 1 << signer, value, created, ready, expires };
            s.proposal_nonce = nonce;
            finish(s, "hub.proposed", 255, value)
        }
        ids::VOTE => {
            let signer = s.config.signer(actor)?;
            Proposal::decode_body(body)?;
            if s.proposal.nonce == 0 || body != s.proposal.body() {
                return Err(E_GOVERNANCE);
            }
            if input.context.block_time >= s.proposal.expires {
                return Err(E_EXPIRED);
            }
            let bit = 1 << signer;
            if s.proposal.votes & bit != 0 {
                return Err(E_DUPLICATE_VOTE);
            }
            s.proposal.votes |= bit;
            finish(s, "hub.voted", 255, snapshot_value(body)?)
        }
        ids::EXECUTE => {
            if body.len() != 8 {
                return Err(E_ARGS);
            }
            let p = s.proposal;
            if p.nonce == 0 || u64_at(body, 0)? != p.nonce {
                return Err(E_GOVERNANCE);
            }
            if input.context.block_time >= p.expires {
                return Err(E_EXPIRED);
            }
            if p.votes.count_ones() < 2 {
                return Err(E_QUORUM);
            }
            if input.context.block_time < p.ready {
                return Err(E_TIMELOCK);
            }
            match p.action {
                PAUSE => s.paused = true,
                RESUME => s.paused = false,
                SET_FUEL => s.fuel_limit = p.value,
                _ => return Err(E_STATE),
            }
            s.proposal = Proposal::default();
            finish(s, "hub.executed", 255, p.value)
        }
        _ => Err(E_ARGS),
    }
}

fn snapshot_value(body: &[u8]) -> HubResult<u128> {
    u128_at(body, 9)
}

fn callback(input: MessageInput<'_>, mut s: State) -> HubResult<MessageOutput> {
    let (Caller::Internal(source), CanonicalMessage::Internal(message)) =
        (input.caller, input.message)
    else {
        return Err(E_CALLBACK);
    };
    let h = message.callback.ok_or(E_CALLBACK)?;
    let i = s.config.members[..usize::from(s.config.count)]
        .iter()
        .position(|m| m.destination == source)
        .ok_or(E_CALLBACK)?;
    let m = s.members[i];
    if m.pending == 0
        || message.method_id != ids::CALLBACK
        || h.cause_address != input.destination
        || h.cause_msg_hash == [0; 32]
        || h.cause_msg_hash == m.last_callback_hash
        || message.created_lt <= m.after_lt
        || message.created_lt <= m.last_callback_lt
        || message.has_callback_request()
        || message.extra.iter().any(|c| c.currency_id != 2)
    {
        return Err(E_CALLBACK);
    }
    let success = h.status_code == 0;
    if success {
        if message.value_main != 0 || message.body != payment_body(i as u8, m.nonce, m.pending) {
            return Err(E_CALLBACK);
        }
        s.members[i].paid = m.paid.checked_add(m.pending).ok_or(E_OVERFLOW)?;
        s.paid = s.paid.checked_add(m.pending).ok_or(E_OVERFLOW)?;
    } else {
        if message.value_main != m.pending || !message.body.is_empty() {
            return Err(E_CALLBACK);
        }
        s.members[i].claimable = m.claimable.checked_add(m.pending).ok_or(E_OVERFLOW)?;
    }
    s.members[i].pending = 0;
    s.members[i].nonce = 0;
    s.members[i].after_lt = 0;
    s.members[i].last_callback_lt = message.created_lt;
    s.members[i].last_callback_hash = h.cause_msg_hash;
    finish(s, if success { "hub.claim_settled" } else { "hub.claim_failed" }, i as u8, m.pending)
}

fn zero_value(input: &MessageInput<'_>) -> HubResult<()> {
    if input.message.value_main() != 0 || !input.message.extra().is_empty() {
        Err(E_VALUE)
    } else {
        Ok(())
    }
}

fn finish(s: State, name: &str, index: u8, amount: u128) -> HubResult<MessageOutput> {
    s.validate()?;
    let fields = vec![
        EventField { name: "amount".into(), value: EventValue::U128(amount) },
        EventField { name: "dust".into(), value: EventValue::U128(s.dust()?) },
        EventField { name: "held".into(), value: EventValue::U128(s.held()?) },
        EventField { name: "member".into(), value: EventValue::U64(u64::from(index)) },
        EventField { name: "nonce".into(), value: EventValue::U64(s.claim_nonce) },
        EventField { name: "paid".into(), value: EventValue::U128(s.paid) },
        EventField { name: "paused".into(), value: EventValue::U64(u64::from(s.paused)) },
        EventField { name: "pending".into(), value: EventValue::U128(s.pending()?) },
        EventField { name: "proposal".into(), value: EventValue::U64(s.proposal_nonce) },
        EventField { name: "received".into(), value: EventValue::U128(s.received) },
    ];
    let mut out = MessageOutput::success(StateUpdate::FullReplace(s.encode()));
    out.event_intents.push(event(0, name, fields)?);
    Ok(out)
}

#[cfg(target_arch = "wasm32")]
avm_contract_sdk::export_message_contract!(RevenueVault, arena_bytes = 262_144);
