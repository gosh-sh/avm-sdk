#![forbid(unsafe_code)]

use avm_contract_sdk::{
    encode_event_fields, AccountAddress, CallbackIntent, Caller, CanonicalMessage, Contract,
    CurrencyAmount, CurrencyAmounts, EventField, EventIntent, EventKind, EventValue, MessageInput,
    MessageOutput, OutboundIntent, StateUpdate,
};

pub mod ids;

/// Synthetic principal selected for this example; no old-main asset conversion.
pub const PRINCIPAL_CURRENCY_ID: u32 = 0;
pub const SHELL_CURRENCY_ID: u32 = 2;

pub fn principal(extra: CurrencyAmounts<'_>) -> u128 {
    extra.iter().find(|c| c.currency_id == PRINCIPAL_CURRENCY_ID).map_or(0, |c| c.amount)
}

pub const CONFIG_BYTES: usize = 360;
pub const STATE_BYTES: usize = 2768;
pub const MAX_MILESTONES: usize = 8;
pub const MAX_ATTEMPTS: usize = 64;
pub const OPEN: u8 = 0;
pub const SUBMITTED: u8 = 1;
pub const APPROVED: u8 = 2;
pub const PAID: u8 = 3;
pub const REFUNDED: u8 = 4;
pub const ERR_ARGS: u32 = 2000;
pub const ERR_AUTH: u32 = 2001;
pub const ERR_STATE: u32 = 2002;
pub const ERR_PHASE: u32 = 2003;
pub const ERR_VALUE: u32 = 2004;
pub const ERR_DEADLINE: u32 = 2005;
pub const ERR_PENDING: u32 = 2006;
pub const ERR_CALLBACK: u32 = 2007;
pub const ERR_LIMIT: u32 = 2008;
pub const ERR_BALANCE: u32 = 2009;

type Result<T> = std::result::Result<T, u32>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub owner: [u8; 32],
    pub worker: [u8; 32],
    pub refund_to: AccountAddress,
    pub pay_to: AccountAddress,
    pub total: u128,
    pub deadline: u32,
    pub count: u8,
    pub amounts: [u128; MAX_MILESTONES],
    pub fuel_reserve: u128,
}

impl Config {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != CONFIG_BYTES || bytes[213..216] != [0; 3] {
            return Err(ERR_ARGS);
        }
        let mut amounts = [0; MAX_MILESTONES];
        for (i, amount) in amounts.iter_mut().enumerate() {
            *amount = read_u128(bytes, 216 + i * 16)?;
        }
        let config = Self {
            owner: array(bytes, 0)?,
            worker: array(bytes, 32)?,
            refund_to: read_address(bytes, 64)?,
            pay_to: read_address(bytes, 128)?,
            total: read_u128(bytes, 192)?,
            deadline: u32::from_le_bytes(array(bytes, 208)?),
            count: bytes[212],
            amounts,
            fuel_reserve: read_u128(bytes, 344)?,
        };
        let n = usize::from(config.count);
        if n == 0
            || n > MAX_MILESTONES
            || config.owner == [0; 32]
            || config.worker == [0; 32]
            || config.owner == config.worker
            || !nonzero_address(config.pay_to)
            || !nonzero_address(config.refund_to)
            || config.pay_to == config.refund_to
            || config.total == 0
            || config.deadline == 0
            || config.fuel_reserve == 0
            || config.amounts[..n].contains(&0)
            || config.amounts[n..].iter().any(|v| *v != 0)
        {
            return Err(ERR_ARGS);
        }
        let sum = config.amounts.iter().try_fold(0u128, |a, b| a.checked_add(*b));
        if sum != Some(config.total) {
            return Err(ERR_ARGS);
        }
        Ok(config)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![0; CONFIG_BYTES];
        b[..32].copy_from_slice(&self.owner);
        b[32..64].copy_from_slice(&self.worker);
        write_address(&mut b, 64, self.refund_to);
        write_address(&mut b, 128, self.pay_to);
        b[192..208].copy_from_slice(&self.total.to_le_bytes());
        b[208..212].copy_from_slice(&self.deadline.to_le_bytes());
        b[212] = self.count;
        for (i, amount) in self.amounts.iter().enumerate() {
            b[216 + i * 16..232 + i * 16].copy_from_slice(&amount.to_le_bytes());
        }
        b[344..360].copy_from_slice(&self.fuel_reserve.to_le_bytes());
        b
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pending {
    pub kind: u8, // 0 idle, 1 milestone payment, 2 expired unapproved balance refund
    pub index: u8,
    pub amount: u128,
    pub attempt: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub config: Config,
    pub funded: u128,
    pub paid: u128,
    pub refunded: u128,
    pub attempts: u64,
    pub pending: Pending,
    pub phases: [u8; MAX_MILESTONES],
    pub delivery_hashes: [[u8; 32]; MAX_MILESTONES],
    pub receipts: Vec<[u8; 32]>,
}

impl State {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            funded: 0,
            paid: 0,
            refunded: 0,
            attempts: 0,
            pending: Pending::default(),
            phases: [OPEN; MAX_MILESTONES],
            delivery_hashes: [[0; 32]; MAX_MILESTONES],
            receipts: vec![],
        }
    }
    pub fn decode(b: &[u8]) -> Result<Self> {
        if b.len() != STATE_BYTES || b[418..424] != [0; 6] {
            return Err(ERR_STATE);
        }
        let receipt_count = u64::from_le_bytes(array(b, 712)?);
        if receipt_count > MAX_ATTEMPTS as u64 {
            return Err(ERR_STATE);
        }
        let mut hashes = [[0; 32]; MAX_MILESTONES];
        for (i, hash) in hashes.iter_mut().enumerate() {
            *hash = array(b, 456 + i * 32)?;
        }
        let mut receipts = Vec::new();
        for i in 0..MAX_ATTEMPTS {
            let hash = array(b, 720 + i * 32)?;
            if i < receipt_count as usize {
                receipts.push(hash);
            } else if hash != [0; 32] {
                return Err(ERR_STATE);
            }
        }
        let state = Self {
            config: Config::decode(&b[..CONFIG_BYTES]).map_err(|_| ERR_STATE)?,
            funded: read_u128(b, 360)?,
            paid: read_u128(b, 376)?,
            refunded: read_u128(b, 392)?,
            attempts: u64::from_le_bytes(array(b, 408)?),
            pending: Pending {
                kind: b[416],
                index: b[417],
                amount: read_u128(b, 424)?,
                attempt: u64::from_le_bytes(array(b, 440)?),
            },
            phases: array(b, 448)?,
            delivery_hashes: hashes,
            receipts,
        };
        state.validate()?;
        Ok(state)
    }
    pub fn validate(&self) -> Result<()> {
        let n = usize::from(self.config.count);
        if self.attempts > MAX_ATTEMPTS as u64
            || self.receipts.len() as u64 + u64::from(self.pending.kind != 0) != self.attempts
            || self
                .receipts
                .iter()
                .enumerate()
                .any(|(i, h)| *h == [0; 32] || self.receipts[..i].contains(h))
            || self.funded != 0 && self.funded != self.config.total
            || self.funded == 0 && (self.attempts != 0 || self.phases != [OPEN; MAX_MILESTONES])
            || self.phases[n..].iter().any(|p| *p != OPEN)
            || self.delivery_hashes[n..].iter().any(|h| *h != [0; 32])
        {
            return Err(ERR_STATE);
        }
        let mut paid = 0u128;
        let mut refunded = 0u128;
        for i in 0..n {
            let p = self.phases[i];
            if p > REFUNDED
                || (p == OPEN && self.delivery_hashes[i] != [0; 32])
                || (matches!(p, SUBMITTED | APPROVED | PAID) && self.delivery_hashes[i] == [0; 32])
            {
                return Err(ERR_STATE);
            }
            if p == PAID {
                paid = paid.checked_add(self.config.amounts[i]).ok_or(ERR_STATE)?;
            }
            if p == REFUNDED {
                refunded = refunded.checked_add(self.config.amounts[i]).ok_or(ERR_STATE)?;
            }
        }
        if paid != self.paid
            || refunded != self.refunded
            || paid.checked_add(refunded).is_none_or(|v| v > self.funded)
        {
            return Err(ERR_STATE);
        }
        let p = self.pending;
        match p.kind {
            0 if p != Pending::default() => return Err(ERR_STATE),
            1 if usize::from(p.index) >= n
                || self.phases[usize::from(p.index)] != APPROVED
                || p.amount != self.config.amounts[usize::from(p.index)] =>
            {
                return Err(ERR_STATE)
            }
            2 if p.index != 255 || p.amount == 0 || p.amount != self.refundable()? => {
                return Err(ERR_STATE)
            }
            0..=2 => {}
            _ => return Err(ERR_STATE),
        }
        if p.kind != 0 && (p.attempt != self.attempts || self.funded == 0) {
            return Err(ERR_STATE);
        }
        Ok(())
    }
    pub fn refundable(&self) -> Result<u128> {
        self.phases[..usize::from(self.config.count)]
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(**p, OPEN | SUBMITTED))
            .try_fold(0u128, |sum, (i, _)| sum.checked_add(self.config.amounts[i]).ok_or(ERR_STATE))
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![0; STATE_BYTES];
        b[..CONFIG_BYTES].copy_from_slice(&self.config.encode());
        for (offset, value) in
            [(360, self.funded), (376, self.paid), (392, self.refunded), (424, self.pending.amount)]
        {
            b[offset..offset + 16].copy_from_slice(&value.to_le_bytes());
        }
        b[408..416].copy_from_slice(&self.attempts.to_le_bytes());
        b[416] = self.pending.kind;
        b[417] = self.pending.index;
        b[440..448].copy_from_slice(&self.pending.attempt.to_le_bytes());
        b[448..456].copy_from_slice(&self.phases);
        for (i, h) in self.delivery_hashes.iter().enumerate() {
            b[456 + i * 32..488 + i * 32].copy_from_slice(h);
        }
        b[712..720].copy_from_slice(&(self.receipts.len() as u64).to_le_bytes());
        for (i, h) in self.receipts.iter().enumerate() {
            b[720 + i * 32..752 + i * 32].copy_from_slice(h);
        }
        b
    }
}

pub struct Escrow;
impl Contract for Escrow {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        wrap(initialize(input))
    }
    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        wrap(process(input))
    }
}

fn wrap(result: Result<MessageOutput>) -> MessageOutput {
    result.unwrap_or_else(MessageOutput::failure)
}

fn initialize(input: MessageInput<'_>) -> Result<MessageOutput> {
    let config = Config::decode(input.deploy().map_err(|_| ERR_ARGS)?.init_data())?;
    if input.caller != Caller::External(config.owner) {
        return Err(ERR_AUTH);
    }
    if !input.state.is_empty() || config.deadline <= input.context.block_time {
        return Err(ERR_ARGS);
    }
    if config.pay_to == input.destination || config.refund_to == input.destination {
        return Err(ERR_ARGS);
    }
    zero_value(&input)?;
    finish(State::new(config), "escrow.created", 255, 0)
}

fn process(input: MessageInput<'_>) -> Result<MessageOutput> {
    let mut state = State::decode(input.state)?;
    if input.message.is_callback() {
        return callback(input, state);
    }
    let Caller::External(actor) = input.caller else {
        return Err(ERR_AUTH);
    };
    let method = input.message.method_id();
    if method == ids::STATUS {
        empty(input.message.body())?;
        zero_value(&input)?;
        return finish(state, "escrow.status", 255, 0);
    }
    if method == ids::TOPUP {
        empty(input.message.body())?;
        if input.message.extra().is_empty()
            || input
                .message
                .extra()
                .iter()
                .any(|c| c.currency_id != SHELL_CURRENCY_ID || c.amount == 0)
        {
            return Err(ERR_VALUE);
        }
        return finish(state, "escrow.fees_added", 255, 0);
    }
    if state.pending.kind != 0 {
        return Err(ERR_PENDING);
    }
    if method == ids::FUND {
        empty(input.message.body())?;
        if actor != state.config.owner {
            return Err(ERR_AUTH);
        }
        if input.context.block_time >= state.config.deadline {
            return Err(ERR_DEADLINE);
        }
        if state.funded != 0 {
            return Err(ERR_PHASE);
        }
        if principal(input.message.extra()) != state.config.total
            || input.message.extra().len() != 1
        {
            return Err(ERR_VALUE);
        }
        if principal(input.extra) < state.config.total {
            return Err(ERR_BALANCE);
        }
        state.funded = state.config.total;
        return finish(state, "escrow.funded", 255, principal(input.message.extra()));
    }
    zero_value(&input)?;
    if state.funded == 0 {
        return Err(ERR_PHASE);
    }
    match method {
        ids::SUBMIT => {
            if actor != state.config.worker {
                return Err(ERR_AUTH);
            }
            if input.context.block_time >= state.config.deadline {
                return Err(ERR_DEADLINE);
            }
            if input.message.body().len() != 33 {
                return Err(ERR_ARGS);
            }
            let index = index(input.message.body()[0], &state)?;
            let hash = array(input.message.body(), 1)?;
            if hash == [0; 32] {
                return Err(ERR_ARGS);
            }
            if !matches!(state.phases[index], OPEN | SUBMITTED) {
                return Err(ERR_PHASE);
            }
            state.phases[index] = SUBMITTED;
            state.delivery_hashes[index] = hash;
            finish(state, "escrow.submitted", index as u8, 0)
        }
        ids::APPROVE => {
            if actor != state.config.owner {
                return Err(ERR_AUTH);
            }
            if input.context.block_time >= state.config.deadline {
                return Err(ERR_DEADLINE);
            }
            // The owner signs the exact delivery hash; a later worker revision cannot
            // silently substitute the submission being approved.
            if input.message.body().len() != 33 {
                return Err(ERR_ARGS);
            }
            let index = index(input.message.body()[0], &state)?;
            if state.phases[index] != SUBMITTED {
                return Err(ERR_PHASE);
            }
            if input.message.body()[1..] != state.delivery_hashes[index] {
                return Err(ERR_ARGS);
            }
            state.phases[index] = APPROVED;
            finish(state, "escrow.approved", index as u8, 0)
        }
        ids::PAY => {
            if actor != state.config.owner && actor != state.config.worker {
                return Err(ERR_AUTH);
            }
            if input.message.body().len() != 1 {
                return Err(ERR_ARGS);
            }
            let i = index(input.message.body()[0], &state)?;
            if state.phases[i] != APPROVED {
                return Err(ERR_PHASE);
            }
            let amount = state.config.amounts[i];
            dispatch(input, state, 1, i as u8, amount)
        }
        ids::REFUND => {
            if actor != state.config.owner {
                return Err(ERR_AUTH);
            }
            empty(input.message.body())?;
            if input.context.block_time < state.config.deadline {
                return Err(ERR_DEADLINE);
            }
            let amount = state.refundable()?;
            if amount == 0 {
                return Err(ERR_PHASE);
            }
            dispatch(input, state, 2, 255, amount)
        }
        _ => Err(ERR_ARGS),
    }
}

fn dispatch(
    input: MessageInput<'_>,
    mut state: State,
    kind: u8,
    index: u8,
    amount: u128,
) -> Result<MessageOutput> {
    if state.attempts >= MAX_ATTEMPTS as u64 {
        return Err(ERR_LIMIT);
    }
    // SHELL fees are funded separately from the example principal ECC.
    if principal(input.extra) < state.funded - state.paid - state.refunded {
        return Err(ERR_BALANCE);
    }
    if !input
        .extra
        .iter()
        .any(|c| c.currency_id == SHELL_CURRENCY_ID && c.amount >= state.config.fuel_reserve)
    {
        return Err(ERR_BALANCE);
    }
    state.attempts += 1;
    state.pending = Pending { kind, index, amount, attempt: state.attempts };
    let destination = if kind == 1 { state.config.pay_to } else { state.config.refund_to };
    let fuel_fee_shells = state.config.fuel_reserve;
    let body = payment_body(state.pending);
    let mut output = finish(state, "escrow.transfer_pending", index, amount)?;
    output.outbound_intents.push(OutboundIntent {
        input_index: 0,
        destination,
        fuel_fee_shells,
        extra: vec![CurrencyAmount { currency_id: PRINCIPAL_CURRENCY_ID, amount }],
        method_id: ids::RECEIVE,
        body,
    });
    output
        .callback_intents
        .push(CallbackIntent { outbound_index: 0, handler_method_id: ids::CALLBACK });
    Ok(output)
}

fn callback(input: MessageInput<'_>, mut state: State) -> Result<MessageOutput> {
    let (Caller::Internal(source), CanonicalMessage::Internal(message)) =
        (input.caller, input.message)
    else {
        return Err(ERR_CALLBACK);
    };
    let header = message.callback.ok_or(ERR_CALLBACK)?;
    let p = state.pending;
    let recipient = if p.kind == 1 { state.config.pay_to } else { state.config.refund_to };
    if p.kind == 0
        || message.method_id != ids::CALLBACK
        || source != recipient
        || header.cause_address != input.destination
        || header.cause_msg_hash == [0; 32]
        || state.receipts.contains(&header.cause_msg_hash)
        || message.has_callback_request()
        || message
            .extra
            .iter()
            .any(|c| c.currency_id != SHELL_CURRENCY_ID && c.currency_id != PRINCIPAL_CURRENCY_ID)
    {
        return Err(ERR_CALLBACK);
    }
    // The runtime, not an ordinary outbound intent, constructs callback headers.
    // Record every accepted causal hash so old success/failure receipts cannot
    // settle a later attempt. Failure callbacks canonically have an empty body.
    let success = header.status_code == 0;
    if success {
        if message.extra.iter().any(|c| c.currency_id != SHELL_CURRENCY_ID)
            || message.body != payment_body(p)
        {
            return Err(ERR_CALLBACK);
        }
        if p.kind == 1 {
            state.phases[usize::from(p.index)] = PAID;
            state.paid = state.paid.checked_add(p.amount).ok_or(ERR_STATE)?;
        } else {
            for phase in &mut state.phases[..usize::from(state.config.count)] {
                if matches!(*phase, OPEN | SUBMITTED) {
                    *phase = REFUNDED;
                }
            }
            state.refunded = state.refunded.checked_add(p.amount).ok_or(ERR_STATE)?;
        }
    } else if principal(message.extra) != p.amount || !message.body.is_empty() {
        // Never release the pending lock without the exact principal refund.
        return Err(ERR_CALLBACK);
    }
    state.receipts.push(header.cause_msg_hash);
    state.pending = Pending::default();
    finish(
        state,
        if success { "escrow.transfer_settled" } else { "escrow.transfer_failed" },
        p.index,
        p.amount,
    )
}

pub fn payment_body(p: Pending) -> Vec<u8> {
    let mut b = Vec::from(*b"BEP1");
    b.extend_from_slice(&[p.kind, p.index, 0, 0]);
    b.extend_from_slice(&p.attempt.to_le_bytes());
    b.extend_from_slice(&p.amount.to_le_bytes());
    b
}

fn finish(state: State, name: &str, index: u8, amount: u128) -> Result<MessageOutput> {
    state.validate()?;
    let fields = encode_event_fields(&[
        EventField { name: "amount".into(), value: EventValue::U128(amount) },
        EventField { name: "attempt".into(), value: EventValue::U64(state.attempts) },
        EventField { name: "funded".into(), value: EventValue::U128(state.funded) },
        EventField { name: "index".into(), value: EventValue::U64(u64::from(index)) },
        EventField { name: "paid".into(), value: EventValue::U128(state.paid) },
        EventField { name: "pending".into(), value: EventValue::U128(state.pending.amount) },
        EventField { name: "refunded".into(), value: EventValue::U128(state.refunded) },
    ])
    .map_err(|_| ERR_STATE)?;
    let mut output = MessageOutput::success(StateUpdate::FullReplace(state.encode()));
    output.event_intents.push(EventIntent {
        input_index: 0,
        kind: EventKind::External,
        event_class: event_class(name)?,
        event_name: name.into(),
        topics: vec![state.config.owner.to_vec()],
        fields_binary: fields,
    });
    Ok(output)
}

fn zero_value(input: &MessageInput<'_>) -> Result<()> {
    if !input.message.extra().is_empty() {
        Err(ERR_VALUE)
    } else {
        Ok(())
    }
}
fn empty(body: &[u8]) -> Result<()> {
    if body.is_empty() {
        Ok(())
    } else {
        Err(ERR_ARGS)
    }
}
fn index(i: u8, state: &State) -> Result<usize> {
    if i < state.config.count {
        Ok(usize::from(i))
    } else {
        Err(ERR_ARGS)
    }
}
pub fn array<const N: usize>(b: &[u8], offset: usize) -> Result<[u8; N]> {
    b.get(offset..offset + N).and_then(|s| s.try_into().ok()).ok_or(ERR_ARGS)
}
pub fn read_u128(b: &[u8], offset: usize) -> Result<u128> {
    array(b, offset).map(u128::from_le_bytes)
}
pub fn read_address(b: &[u8], offset: usize) -> Result<AccountAddress> {
    Ok(AccountAddress { dapp_id: array(b, offset)?, account_id: array(b, offset + 32)? })
}
pub fn write_address(b: &mut [u8], offset: usize, a: AccountAddress) {
    b[offset..offset + 32].copy_from_slice(&a.dapp_id);
    b[offset + 32..offset + 64].copy_from_slice(&a.account_id);
}
fn nonzero_address(a: AccountAddress) -> bool {
    a.dapp_id != [0; 32] && a.account_id != [0; 32]
}

#[cfg(all(target_arch = "wasm32", feature = "guest"))]
avm_contract_sdk::export_message_contract!(Escrow, arena_bytes = 262_144);

fn event_class(name: &str) -> Result<u64> {
    match name {
        "escrow.created" => Ok(ids::EVENT_CREATED),
        "escrow.status" => Ok(ids::EVENT_STATUS),
        "escrow.fees_added" => Ok(ids::EVENT_FEES_ADDED),
        "escrow.funded" => Ok(ids::EVENT_FUNDED),
        "escrow.submitted" => Ok(ids::EVENT_SUBMITTED),
        "escrow.approved" => Ok(ids::EVENT_APPROVED),
        "escrow.transfer_pending" => Ok(ids::EVENT_TRANSFER_PENDING),
        "escrow.transfer_settled" => Ok(ids::EVENT_TRANSFER_SETTLED),
        "escrow.transfer_failed" => Ok(ids::EVENT_TRANSFER_FAILED),
        _ => Err(ERR_STATE),
    }
}
