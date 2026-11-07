#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(target_arch = "wasm32", deny(unsafe_code), no_std)]

//! Multisig treasury with a timelock, spending policies and a circuit breaker.
//!
//! A fixed set of owners (up to eight, identified by their external keys) holds
//! multi-currency ECC funds. Nothing moves and nothing about the governance
//! changes except through a *proposal* that collects at least `threshold`
//! approvals and then waits out a delay:
//!
//! * transfers send funds at execution and remain pending until a valid
//!   callback confirms delivery or returns the principal,
//! * per-currency policies let small transfers skip the delay (fast lane) and
//!   cap what may leave per time window,
//! * owners, the threshold, the delay and the policies are all changed by
//!   proposals of their own,
//! * any single owner can freeze the treasury; only a threshold-approved
//!   unfreeze proposal lifts it.

extern crate alloc;

use alloc::{vec, vec::Vec};

use avm_contract_sdk::{
    encode_event_fields, AccountAddress, CallbackIntent, Caller, CanonicalMessage, Contract,
    CurrencyAmount, CurrencyAmounts, EventField, EventIntent, EventKind, EventValue, MessageInput,
    MessageOutput, OutboundIntent, StateUpdate,
};

pub mod ids;
#[cfg(all(target_arch = "wasm32", feature = "guest"))]
#[allow(unsafe_code)]
mod rt;
pub mod state;

pub use state::{
    Balance, Init, Pending, Policy, Proposal, State, KIND_ADD_OWNER, KIND_REMOVE_OWNER,
    KIND_SET_DELAY, KIND_SET_POLICY, KIND_SET_THRESHOLD, KIND_TRANSFER, KIND_UNFREEZE,
    MAX_BALANCES, MAX_OWNERS, MAX_PENDING, MAX_POLICIES, MAX_PROPOSALS, NO_PROPOSER, RECEIPT_SLOTS,
    STATE_BYTES, STATUS_EXECUTING, STATUS_OPEN, STATUS_QUEUED,
};

/// SHELL (ECC 2) pays for execution; it can be deposited as FUEL but never paid out.
pub const SHELL_CURRENCY_ID: u32 = 2;
/// The longest timelock an owner set can configure: 30 days.
pub const MAX_DELAY: u32 = 2_592_000;

pub const ERR_ARGS: u32 = 4000;
pub const ERR_AUTH: u32 = 4001;
pub const ERR_STATE: u32 = 4002;
pub const ERR_FROZEN: u32 = 4003;
pub const ERR_VALUE: u32 = 4004;
pub const ERR_NOT_FOUND: u32 = 4005;
pub const ERR_EXPIRED: u32 = 4006;
pub const ERR_TIMELOCK: u32 = 4007;
pub const ERR_LIMIT: u32 = 4008;
pub const ERR_BALANCE: u32 = 4009;
pub const ERR_CAPACITY: u32 = 4010;
pub const ERR_INSOLVENT: u32 = 4011;
pub const ERR_CALLBACK: u32 = 4012;
pub const ERR_DUPLICATE: u32 = 4013;
pub const ERR_THRESHOLD: u32 = 4014;
pub const ERR_OVERFLOW: u32 = 4015;
pub const ERR_PENDING: u32 = 4016;
pub const ERR_NOT_READY: u32 = 4017;

pub type Result<T> = core::result::Result<T, u32>;

pub const PROPOSE_ARGS_BYTES: usize = 89;
pub const ID_ARGS_BYTES: usize = 8;

pub struct Treasury;

impl Contract for Treasury {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        initialize(input).unwrap_or_else(MessageOutput::failure)
    }

    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        process(input).unwrap_or_else(MessageOutput::failure)
    }
}

/// The 32-byte receipt sent with a payout and echoed by an accepting receiver:
/// `TRS1 | currency u32 | proposal id u64 | amount u128` (little endian).
pub fn payout_receipt(currency: u32, id: u64, amount: u128) -> Vec<u8> {
    let mut body = Vec::from(*b"TRS1");
    body.extend_from_slice(&currency.to_le_bytes());
    body.extend_from_slice(&id.to_le_bytes());
    body.extend_from_slice(&amount.to_le_bytes());
    body
}

// ---------------------------------------------------------------- events ---

/// One event. Field names must be added in ascending order: the SDK rejects
/// anything else, and the tests exercise every event.
struct Ev {
    name: &'static str,
    topic: [u8; 32],
    fields: Vec<EventField>,
}

impl Ev {
    fn new(name: &'static str, topic: [u8; 32]) -> Self {
        Self { name, topic, fields: Vec::new() }
    }
    fn u128(mut self, name: &str, value: u128) -> Self {
        self.fields.push(EventField { name: name.into(), value: EventValue::U128(value) });
        self
    }
    fn u64(mut self, name: &str, value: u64) -> Self {
        self.fields.push(EventField { name: name.into(), value: EventValue::U64(value) });
        self
    }
    fn key(mut self, name: &str, value: [u8; 32]) -> Self {
        self.fields.push(EventField { name: name.into(), value: EventValue::Data(value.to_vec()) });
        self
    }
}

/// Validates the next state and wraps it, with its events, into a success output.
fn finish(state: &mut State, events: Vec<Ev>) -> Result<MessageOutput> {
    state.ops = state.ops.saturating_add(1);
    state.validate()?;
    let mut output = MessageOutput::success(StateUpdate::FullReplace(state.encode()));
    for Ev { name, topic, fields } in events {
        let event_class = ids::EVENTS
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, id)| *id)
            .ok_or(ERR_STATE)?;
        output.event_intents.push(EventIntent {
            input_index: 0,
            kind: EventKind::External,
            event_class,
            event_name: name.into(),
            topics: vec![topic.to_vec()],
            fields_binary: encode_event_fields(&fields).map_err(|_| ERR_STATE)?,
        });
    }
    Ok(output)
}

// --------------------------------------------------------------- helpers ---

fn array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N]> {
    bytes.get(offset..offset + N).and_then(|slice| slice.try_into().ok()).ok_or(ERR_ARGS)
}
fn read_u128(bytes: &[u8], offset: usize) -> Result<u128> {
    array(bytes, offset).map(u128::from_le_bytes)
}
fn read_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    array(bytes, offset).map(u64::from_le_bytes)
}
fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    array(bytes, offset).map(u32::from_le_bytes)
}
fn amount_of(extra: CurrencyAmounts<'_>, currency: u32) -> u128 {
    extra.iter().find(|entry| entry.currency_id == currency).map_or(0, |entry| entry.amount)
}
fn address_of(target: &[u8; 64]) -> AccountAddress {
    AccountAddress {
        dapp_id: array(target, 0).unwrap_or([0; 32]),
        account_id: array(target, 32).unwrap_or([0; 32]),
    }
}
fn all_zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| *byte == 0)
}

/// The treasury must physically hold everything it believes it owns. The check
/// holds whether `input.extra` is the balance before or after this message's own
/// transfer, so it never depends on that detail.
fn check_solvency(state: &State, balance: CurrencyAmounts<'_>) -> Result<()> {
    for entry in &state.balances {
        if amount_of(balance, entry.currency) < entry.held {
            return Err(ERR_INSOLVENT);
        }
    }
    Ok(())
}

fn open_deadline(created: u32, ttl: u32) -> u32 {
    created.saturating_add(ttl)
}

// ------------------------------------------------------------------ init ---

fn initialize(input: MessageInput<'_>) -> Result<MessageOutput> {
    let deploy = input.deploy().map_err(|_| ERR_ARGS)?;
    let init = Init::parse(deploy.init_data()).map_err(|_| ERR_ARGS)?;
    let Caller::External(deployer) = input.caller else {
        return Err(ERR_AUTH);
    };
    if !init.owners.contains(&deployer) {
        return Err(ERR_AUTH);
    }
    if !input.state.is_empty() || !input.message.extra().is_empty() {
        return Err(ERR_ARGS);
    }
    let mut state = State::new(&init).map_err(|_| ERR_ARGS)?;
    let event = Ev::new("msig.created", deployer)
        .u64("delay", u64::from(init.delay))
        .u128("fuel_reserve", init.fuel_reserve)
        .u64("grace", u64::from(init.grace))
        .u64("owners", init.owners.len() as u64)
        .u64("threshold", u64::from(init.threshold))
        .u64("ttl", u64::from(init.ttl));
    finish(&mut state, vec![event])
}

// --------------------------------------------------------------- process ---

fn process(input: MessageInput<'_>) -> Result<MessageOutput> {
    let mut state = State::decode(input.state)?;
    if input.message.is_callback() {
        return callback(&input, state);
    }
    check_solvency(&state, input.extra)?;
    let now = input.context.block_time;
    let body = input.message.body();
    let value = input.message.extra();
    let method = input.message.method_id();
    if method == ids::DEPOSIT {
        if !body.is_empty() {
            return Err(ERR_ARGS);
        }
        let events = deposit(&mut state, &input, value)?;
        return finish(&mut state, events);
    }
    let Caller::External(actor) = input.caller else {
        return Err(ERR_AUTH);
    };
    let slot = state.owner_slot(&actor).ok_or(ERR_AUTH)?;
    if !value.is_empty() {
        return Err(ERR_VALUE);
    }
    let events = match method {
        ids::PROPOSE => propose(&mut state, slot, body, now, input.destination)?,
        ids::APPROVE => approve(&mut state, slot, id_arg(body)?, now)?,
        ids::REVOKE => revoke(&mut state, slot, id_arg(body)?, now)?,
        ids::CANCEL => cancel(&mut state, slot, id_arg(body)?, now)?,
        ids::EXECUTE => return execute(state, slot, id_arg(body)?, now, &input),
        ids::FREEZE => freeze(&mut state, slot, body)?,
        _ => return Err(ERR_ARGS),
    };
    finish(&mut state, events)
}

fn id_arg(body: &[u8]) -> Result<u64> {
    if body.len() != crate::ID_ARGS_BYTES {
        return Err(ERR_ARGS);
    }
    read_u64(body, 0)
}

fn deposit(
    state: &mut State,
    input: &MessageInput<'_>,
    value: CurrencyAmounts<'_>,
) -> Result<Vec<Ev>> {
    let from = match input.caller {
        Caller::External(key) => key,
        Caller::Internal(address) => address.account_id,
    };
    let mut events = Vec::new();
    for entry in value.iter() {
        if entry.amount == 0 {
            return Err(ERR_VALUE);
        }
        state.credit(entry.currency_id, entry.amount)?;
        events.push(
            Ev::new("msig.deposited", from)
                .u128("amount", entry.amount)
                .u64("currency", u64::from(entry.currency_id))
                .key("from", from),
        );
    }
    if events.is_empty() {
        return Err(ERR_VALUE);
    }
    Ok(events)
}

fn freeze(state: &mut State, slot: usize, body: &[u8]) -> Result<Vec<Ev>> {
    if !body.is_empty() {
        return Err(ERR_ARGS);
    }
    if state.frozen {
        return Err(ERR_STATE);
    }
    state.frozen = true;
    let by = state.owners[slot];
    Ok(vec![Ev::new("msig.frozen", by).key("by", by)])
}

// ------------------------------------------------------------- proposals ---

/// Static and state-dependent validity of a proposal's payload. It runs when a
/// proposal is created and again right before it executes, because the owner
/// set, the threshold and the balances may have changed in between.
fn check_payload(state: &State, proposal: &Proposal, me: AccountAddress) -> Result<()> {
    let Proposal { kind, currency, aux, amount, target, .. } = *proposal;
    let key: [u8; 32] = array(&target, 0)?;
    let single_key = key != [0; 32] && all_zero(&target[32..]);
    let no_numbers = currency == 0 && aux == 0 && amount == 0;
    match kind {
        KIND_TRANSFER => {
            let to = address_of(&target);
            if amount == 0
                || currency == SHELL_CURRENCY_ID
                || aux != 0
                || !state::nonzero(to)
                || to == me
            {
                return Err(ERR_ARGS);
            }
        }
        KIND_ADD_OWNER => {
            if !single_key || !no_numbers {
                return Err(ERR_ARGS);
            }
            if state.owner_slot(&key).is_some() {
                return Err(ERR_DUPLICATE);
            }
            if state.owner_count() >= MAX_OWNERS {
                return Err(ERR_CAPACITY);
            }
        }
        KIND_REMOVE_OWNER => {
            if !single_key || !no_numbers {
                return Err(ERR_ARGS);
            }
            if state.owner_slot(&key).is_none() {
                return Err(ERR_NOT_FOUND);
            }
            if state.owner_count() - 1 < usize::from(state.threshold) {
                return Err(ERR_THRESHOLD);
            }
        }
        KIND_SET_THRESHOLD => {
            if currency != 0 || amount != 0 || !all_zero(&target) {
                return Err(ERR_ARGS);
            }
            if aux == 0 || aux as usize > state.owner_count() {
                return Err(ERR_THRESHOLD);
            }
        }
        KIND_SET_DELAY => {
            if currency != 0 || amount != 0 || !all_zero(&target) || aux > MAX_DELAY {
                return Err(ERR_ARGS);
            }
        }
        KIND_SET_POLICY => {
            // amount = fast limit, aux = window seconds, target[..16] = window limit.
            let limit = read_u128(&target, 0)?;
            if currency == SHELL_CURRENCY_ID || !all_zero(&target[16..]) {
                return Err(ERR_ARGS);
            }
            if aux == 0 {
                if amount != 0 || limit != 0 {
                    return Err(ERR_ARGS);
                }
                if state.policy(currency).is_none() {
                    return Err(ERR_NOT_FOUND);
                }
            } else if limit == 0 || amount > limit {
                return Err(ERR_ARGS);
            }
        }
        KIND_UNFREEZE => {
            if !no_numbers || !all_zero(&target) {
                return Err(ERR_ARGS);
            }
            if !state.frozen {
                return Err(ERR_STATE);
            }
        }
        _ => return Err(ERR_ARGS),
    }
    Ok(())
}

/// Seconds a proposal waits once it has enough approvals. Small transfers inside
/// a currency's fast lane and unfreezing skip the delay; governance never does.
fn effective_delay(state: &State, proposal: &Proposal) -> u32 {
    match proposal.kind {
        KIND_UNFREEZE => 0,
        KIND_TRANSFER => match state.policy(proposal.currency) {
            Some(policy) if proposal.amount <= policy.fast_limit => 0,
            _ => state.delay,
        },
        _ => state.delay,
    }
}

/// Moves an open proposal with enough approvals into the timelock queue.
fn queue(state: &mut State, index: usize, now: u32) -> Result<Ev> {
    let delay = effective_delay(state, &state.proposals[index]);
    let eta = now.checked_add(delay).ok_or(ERR_OVERFLOW)?;
    let expires = eta.checked_add(state.grace).ok_or(ERR_OVERFLOW)?;
    let proposal = &mut state.proposals[index];
    proposal.status = STATUS_QUEUED;
    proposal.eta = eta;
    proposal.expires = expires;
    let owner = [0; 32];
    Ok(Ev::new("msig.queued", owner)
        .u64("eta", u64::from(eta))
        .u64("expires", u64::from(expires))
        .u64("id", proposal.id))
}

/// Re-establishes the approval invariants after the owner set or the threshold
/// changed: approvals of removed owners vanish, queued proposals that no longer
/// have enough approvals reopen, and open ones that now do are queued.
fn reconcile(state: &mut State, now: u32) -> Result<()> {
    let mask = state.owner_mask();
    for index in 0..state.proposals.len() {
        if state.proposals[index].status == STATUS_EXECUTING {
            continue;
        }
        let ttl = state.ttl;
        let threshold = u32::from(state.threshold);
        let proposal = &mut state.proposals[index];
        proposal.approvals &= mask;
        let enough = proposal.approvals.count_ones() >= threshold;
        if proposal.status == STATUS_QUEUED && !enough {
            proposal.status = STATUS_OPEN;
            proposal.eta = 0;
            proposal.expires = open_deadline(proposal.created, ttl);
        } else if proposal.status == STATUS_OPEN && enough && now <= proposal.expires {
            queue(state, index, now)?;
        }
    }
    Ok(())
}

fn propose(
    state: &mut State,
    slot: usize,
    body: &[u8],
    now: u32,
    me: AccountAddress,
) -> Result<Vec<Ev>> {
    if body.len() != PROPOSE_ARGS_BYTES {
        return Err(ERR_ARGS);
    }
    let kind = body[0];
    if state.frozen && kind != KIND_UNFREEZE {
        return Err(ERR_FROZEN);
    }
    if state.proposals.len() >= MAX_PROPOSALS {
        return Err(ERR_CAPACITY);
    }
    let id = state.next_id;
    let proposal = Proposal {
        id,
        kind,
        status: STATUS_OPEN,
        proposer: slot as u8,
        approvals: 1 << slot,
        created: now,
        eta: 0,
        expires: now.checked_add(state.ttl).ok_or(ERR_OVERFLOW)?,
        currency: read_u32(body, 1)?,
        aux: read_u32(body, 5)?,
        amount: read_u128(body, 9)?,
        target: array(body, 25)?,
    };
    check_payload(state, &proposal, me)?;
    state.next_id = id.checked_add(1).ok_or(ERR_OVERFLOW)?;
    state.proposals.push(proposal);
    let owner = state.owners[slot];
    let mut events = vec![Ev::new("msig.proposed", owner)
        .u128("amount", proposal.amount)
        .u64("currency", u64::from(proposal.currency))
        .u64("id", id)
        .u64("kind", u64::from(kind))
        .key("proposer", owner)];
    if u32::from(state.threshold) <= 1 {
        let index = state.proposals.len() - 1;
        events.push(queue(state, index, now)?);
    }
    Ok(events)
}

fn find(state: &State, id: u64) -> Result<usize> {
    state.proposal_index(id).ok_or(ERR_NOT_FOUND)
}

fn approve(state: &mut State, slot: usize, id: u64, now: u32) -> Result<Vec<Ev>> {
    let index = find(state, id)?;
    let proposal = state.proposals[index];
    if state.frozen && proposal.kind != KIND_UNFREEZE {
        return Err(ERR_FROZEN);
    }
    if proposal.status == STATUS_EXECUTING {
        return Err(ERR_NOT_READY);
    }
    if now > proposal.expires {
        return Err(ERR_EXPIRED);
    }
    if proposal.approvals & (1 << slot) != 0 {
        return Err(ERR_DUPLICATE);
    }
    state.proposals[index].approvals |= 1 << slot;
    let approvals = state.proposals[index].approvals;
    let owner = state.owners[slot];
    let mut events = vec![Ev::new("msig.approved", owner)
        .u64("approvals", u64::from(approvals.count_ones()))
        .u64("id", id)
        .key("owner", owner)];
    if proposal.status == STATUS_OPEN && approvals.count_ones() >= u32::from(state.threshold) {
        events.push(queue(state, index, now)?);
    }
    Ok(events)
}

fn revoke(state: &mut State, slot: usize, id: u64, now: u32) -> Result<Vec<Ev>> {
    let index = find(state, id)?;
    let proposal = state.proposals[index];
    if proposal.status == STATUS_EXECUTING {
        return Err(ERR_NOT_READY);
    }
    if proposal.approvals & (1 << slot) == 0 {
        return Err(ERR_NOT_FOUND);
    }
    state.proposals[index].approvals &= !(1 << slot);
    reconcile(state, now)?;
    let approvals = state.proposals[index].approvals;
    let owner = state.owners[slot];
    Ok(vec![Ev::new("msig.revoked", owner)
        .u64("approvals", u64::from(approvals.count_ones()))
        .u64("id", id)
        .key("owner", owner)])
}

/// The proposer can withdraw a proposal at any time before it starts executing.
/// Any owner can clear one that expired or whose proposer is gone.
fn cancel(state: &mut State, slot: usize, id: u64, now: u32) -> Result<Vec<Ev>> {
    let index = find(state, id)?;
    let proposal = state.proposals[index];
    if proposal.status == STATUS_EXECUTING {
        return Err(ERR_NOT_READY);
    }
    if proposal.proposer != slot as u8
        && proposal.proposer != NO_PROPOSER
        && now <= proposal.expires
    {
        return Err(ERR_AUTH);
    }
    state.proposals.remove(index);
    let by = state.owners[slot];
    Ok(vec![Ev::new("msig.cancelled", by).key("by", by).u64("id", id)])
}

fn execute(
    mut state: State,
    slot: usize,
    id: u64,
    now: u32,
    input: &MessageInput<'_>,
) -> Result<MessageOutput> {
    let index = find(&state, id)?;
    let proposal = state.proposals[index];
    if state.frozen && proposal.kind != KIND_UNFREEZE {
        return Err(ERR_FROZEN);
    }
    if proposal.status != STATUS_QUEUED {
        return Err(ERR_NOT_READY);
    }
    if now < proposal.eta {
        return Err(ERR_TIMELOCK);
    }
    if now > proposal.expires {
        return Err(ERR_EXPIRED);
    }
    check_payload(&state, &proposal, input.destination)?;
    let caller = state.owners[slot];
    if proposal.kind == KIND_TRANSFER {
        return start_transfer(state, index, now, input);
    }
    state.proposals.remove(index);
    let target_key: [u8; 32] = array(&proposal.target, 0)?;
    let event = match proposal.kind {
        KIND_ADD_OWNER => {
            let free = state.owners.iter().position(|key| *key == [0; 32]).ok_or(ERR_CAPACITY)?;
            state.owners[free] = target_key;
            Ev::new("msig.owner_added", caller).u64("id", id).key("owner", target_key)
        }
        KIND_REMOVE_OWNER => {
            let removed = state.owner_slot(&target_key).ok_or(ERR_NOT_FOUND)?;
            state.owners[removed] = [0; 32];
            for other in state.proposals.iter_mut() {
                if other.proposer == removed as u8 {
                    other.proposer = NO_PROPOSER;
                }
            }
            reconcile(&mut state, now)?;
            Ev::new("msig.owner_removed", caller).u64("id", id).key("owner", target_key)
        }
        KIND_SET_THRESHOLD => {
            state.threshold = proposal.aux as u8;
            reconcile(&mut state, now)?;
            Ev::new("msig.threshold_changed", caller)
                .u64("id", id)
                .u64("threshold", u64::from(proposal.aux))
        }
        KIND_SET_DELAY => {
            state.delay = proposal.aux;
            Ev::new("msig.delay_changed", caller)
                .u64("delay", u64::from(proposal.aux))
                .u64("id", id)
        }
        KIND_SET_POLICY => {
            let limit = read_u128(&proposal.target, 0)?;
            state.policies.retain(|policy| policy.currency != proposal.currency);
            if proposal.aux != 0 {
                if state.policies.len() >= MAX_POLICIES {
                    return Err(ERR_CAPACITY);
                }
                state.policies.push(Policy {
                    currency: proposal.currency,
                    window_secs: proposal.aux,
                    fast_limit: proposal.amount,
                    window_limit: limit,
                    window_start: 0,
                    spent: 0,
                });
            }
            Ev::new("msig.policy_set", caller)
                .u64("currency", u64::from(proposal.currency))
                .u128("fast_limit", proposal.amount)
                .u64("id", id)
                .u128("window_limit", limit)
                .u64("window_secs", u64::from(proposal.aux))
        }
        _ => {
            state.frozen = false;
            Ev::new("msig.unfrozen", caller).u64("id", id)
        }
    };
    finish(&mut state, vec![event])
}

/// Debits the treasury and pays the recipient, tracked until its callback.
fn start_transfer(
    mut state: State,
    index: usize,
    now: u32,
    input: &MessageInput<'_>,
) -> Result<MessageOutput> {
    let proposal = state.proposals[index];
    let recipient = address_of(&proposal.target);
    if state.pending.len() >= MAX_PENDING {
        return Err(ERR_CAPACITY);
    }
    if state.pending.iter().any(|entry| entry.recipient == recipient) {
        return Err(ERR_PENDING);
    }
    let fuel = state.fuel_reserve;
    if state.held(proposal.currency) < proposal.amount {
        return Err(ERR_BALANCE);
    }
    if state.held(SHELL_CURRENCY_ID) < fuel {
        return Err(ERR_INSOLVENT);
    }
    let mut window_start = 0;
    if let Some(policy) = state.policies.iter_mut().find(|p| p.currency == proposal.currency) {
        if now >= policy.window_start.saturating_add(policy.window_secs) {
            policy.window_start = now;
            policy.spent = 0;
        }
        let spent = policy.spent.checked_add(proposal.amount).ok_or(ERR_OVERFLOW)?;
        if spent > policy.window_limit {
            return Err(ERR_LIMIT);
        }
        policy.spent = spent;
        window_start = policy.window_start;
    }
    state.debit(proposal.currency, proposal.amount)?;
    state.debit(SHELL_CURRENCY_ID, fuel)?;
    state.proposals[index].status = STATUS_EXECUTING;
    state.proposals[index].aux = window_start;
    state.pending.push(Pending {
        recipient,
        id: proposal.id,
        currency: proposal.currency,
        amount: proposal.amount,
    });
    let by = match input.caller {
        Caller::External(key) => key,
        Caller::Internal(_) => [0; 32],
    };
    let event = Ev::new("msig.transfer_pending", by)
        .u128("amount", proposal.amount)
        .u64("currency", u64::from(proposal.currency))
        .u64("id", proposal.id);
    let mut output = finish(&mut state, vec![event])?;
    output.outbound_intents.push(OutboundIntent {
        input_index: 0,
        destination: recipient,
        fuel_fee_shells: fuel,
        extra: vec![CurrencyAmount { currency_id: proposal.currency, amount: proposal.amount }],
        method_id: ids::PAYEE_RECEIVE,
        body: payout_receipt(proposal.currency, proposal.id, proposal.amount),
    });
    output
        .callback_intents
        .push(CallbackIntent { outbound_index: 0, handler_method_id: ids::CALLBACK });
    Ok(output)
}

/// Settles or refunds the transfer addressed to the callback's source.
fn callback(input: &MessageInput<'_>, mut state: State) -> Result<MessageOutput> {
    let (Caller::Internal(source), CanonicalMessage::Internal(message)) =
        (input.caller, input.message)
    else {
        return Err(ERR_CALLBACK);
    };
    let header = message.callback.ok_or(ERR_CALLBACK)?;
    if message.method_id != ids::CALLBACK
        || header.cause_address != input.destination
        || header.cause_msg_hash == [0; 32]
        || state.receipt_seen(&header.cause_msg_hash)
        || message.has_callback_request()
    {
        return Err(ERR_CALLBACK);
    }
    // One in-flight payout per recipient makes the receiver address a unique key.
    let position =
        state.pending.iter().position(|entry| entry.recipient == source).ok_or(ERR_CALLBACK)?;
    let entry = state.pending[position];
    if message
        .extra
        .iter()
        .any(|c| c.currency_id != SHELL_CURRENCY_ID && c.currency_id != entry.currency)
    {
        return Err(ERR_CALLBACK);
    }
    let returned = amount_of(message.extra, entry.currency);
    let receipt = payout_receipt(entry.currency, entry.id, entry.amount);
    let success = header.status_code == 0;
    if success {
        // Delivered: the receiver must echo the receipt and return no principal.
        if returned != 0 || message.body != receipt.as_slice() {
            return Err(ERR_CALLBACK);
        }
    } else if returned != entry.amount || !message.body.is_empty() {
        // Bounced: never release the slot without the exact principal.
        return Err(ERR_CALLBACK);
    }
    let now = input.context.block_time;
    let index = state.proposal_index(entry.id).ok_or(ERR_STATE)?;
    state.credit(SHELL_CURRENCY_ID, amount_of(message.extra, SHELL_CURRENCY_ID))?;
    let by = [0; 32];
    let name = if success {
        state.proposals.remove(index);
        "msig.transfer_settled"
    } else {
        state.credit(entry.currency, entry.amount)?;
        let proposal = &mut state.proposals[index];
        let window_start = proposal.aux;
        proposal.status = STATUS_QUEUED;
        proposal.eta = now;
        proposal.expires = now.saturating_add(state.grace);
        proposal.aux = 0;
        if let Some(policy) = state.policies.iter_mut().find(|p| p.currency == entry.currency) {
            if policy.window_start == window_start {
                policy.spent = policy.spent.saturating_sub(entry.amount);
            }
        }
        "msig.transfer_failed"
    };
    state.pending.remove(position);
    state.push_receipt(header.cause_msg_hash);
    reconcile(&mut state, now)?;
    let event = Ev::new(name, by)
        .u128("amount", entry.amount)
        .u64("currency", u64::from(entry.currency))
        .u64("id", entry.id);
    finish(&mut state, vec![event])
}

#[cfg(all(target_arch = "wasm32", feature = "guest"))]
avm_contract_sdk::export_message_contract!(Treasury, arena_bytes = 262_144);
