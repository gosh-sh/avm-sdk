#![forbid(unsafe_code)]

//! Vault AMM: a constant-product pool for two ECC currencies.
//!
//! Holders deposit ECC into an internal ledger, trade and provide liquidity
//! against it, and withdraw to any account through an acknowledged payout. The
//! ledger design keeps every user operation synchronous and independent: one
//! unresolved payout never blocks other holders, and a bounced payout returns
//! to its holder's balance.

use avm_contract_sdk::{
    encode_event_fields, AccountAddress, CallbackIntent, Caller, CanonicalMessage, Contract,
    CurrencyAmount, CurrencyAmounts, EventField, EventIntent, EventKind, EventValue, MessageInput,
    MessageOutput, OutboundIntent, StateUpdate,
};

pub mod ids;
pub mod math;
pub mod state;

pub use state::{
    Account, Config, Pending, State, ACCOUNT_SLOTS, CONFIG_BYTES, PENDING_SLOTS, RECEIPT_SLOTS,
    STATE_BYTES,
};

/// SHELL (ECC 2) pays for execution and is never a pool asset.
pub const SHELL_CURRENCY_ID: u32 = 2;
/// LP shares permanently locked by the first deposit, as in Uniswap v2.
pub const MINIMUM_LIQUIDITY: u128 = 1_000;
pub const MAX_SWAP_FEE_BPS: u16 = 1_000;
pub const MAX_PROTOCOL_FEE_BPS: u16 = 5_000;

pub const ERR_ARGS: u32 = 3000;
pub const ERR_AUTH: u32 = 3001;
pub const ERR_STATE: u32 = 3002;
pub const ERR_PAUSED: u32 = 3003;
pub const ERR_VALUE: u32 = 3004;
pub const ERR_DEADLINE: u32 = 3005;
pub const ERR_SLIPPAGE: u32 = 3006;
pub const ERR_BALANCE: u32 = 3007;
pub const ERR_NO_LIQUIDITY: u32 = 3008;
pub const ERR_PENDING: u32 = 3009;
pub const ERR_CALLBACK: u32 = 3010;
pub const ERR_CAPACITY: u32 = 3011;
pub const ERR_INSOLVENT: u32 = 3012;
pub const ERR_OVERFLOW: u32 = 3013;
pub const ERR_INVARIANT: u32 = 3014;
pub const ERR_ZERO: u32 = 3015;

pub type Result<T> = core::result::Result<T, u32>;

pub const SWAP_ARGS_BYTES: usize = 37;
pub const ADD_LIQUIDITY_ARGS_BYTES: usize = 68;
pub const REMOVE_LIQUIDITY_ARGS_BYTES: usize = 52;
pub const WITHDRAW_ARGS_BYTES: usize = 84;
pub const SET_PARAMS_ARGS_BYTES: usize = 5;
pub const PAYOUT_RECEIPT_BYTES: usize = 32;

pub struct Amm;

impl Contract for Amm {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        initialize(input).unwrap_or_else(MessageOutput::failure)
    }

    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        process(input).unwrap_or_else(MessageOutput::failure)
    }
}

/// The 32-byte receipt sent with a payout and echoed by an accepting receiver:
/// `VAM1 | currency u32 | id u64 | amount u128` (little endian).
pub fn payout_receipt(currency: u32, id: u64, amount: u128) -> Vec<u8> {
    let mut body = Vec::from(*b"VAM1");
    body.extend_from_slice(&currency.to_le_bytes());
    body.extend_from_slice(&id.to_le_bytes());
    body.extend_from_slice(&amount.to_le_bytes());
    body
}

// ---------------------------------------------------------------- events ---

struct Ev {
    name: &'static str,
    topics: Vec<Vec<u8>>,
    fields: Vec<EventField>,
}

impl Ev {
    fn new(name: &'static str) -> Self {
        Self { name, topics: Vec::new(), fields: Vec::new() }
    }
    fn topic(mut self, key: [u8; 32]) -> Self {
        self.topics.push(key.to_vec());
        self
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

/// Validates the next state and wraps it, with its event, into a success output.
fn finish(state: &State, event: Ev) -> Result<MessageOutput> {
    state.validate()?;
    let Ev { name, topics, fields } = event;
    // Every event adds its fields in strictly ascending name order, as the SDK
    // requires, so no runtime sort is needed.
    debug_assert!(fields.windows(2).all(|pair| pair[0].name.as_bytes() < pair[1].name.as_bytes()));
    let fields_binary = encode_event_fields(&fields).map_err(|_| ERR_STATE)?;
    let event_class =
        ids::EVENTS.iter().find(|(known, _)| *known == name).map(|(_, id)| *id).ok_or(ERR_STATE)?;
    let mut output = MessageOutput::success(StateUpdate::FullReplace(state.encode()));
    output.event_intents.push(EventIntent {
        input_index: 0,
        kind: EventKind::External,
        event_class,
        event_name: name.into(),
        topics,
        fields_binary,
    });
    Ok(output)
}

// --------------------------------------------------------------- helpers ---

fn array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N]> {
    bytes.get(offset..offset + N).and_then(|slice| slice.try_into().ok()).ok_or(ERR_ARGS)
}
fn read_u128(bytes: &[u8], offset: usize) -> Result<u128> {
    array(bytes, offset).map(u128::from_le_bytes)
}
fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    array(bytes, offset).map(u32::from_le_bytes)
}
fn read_address(bytes: &[u8], offset: usize) -> Result<AccountAddress> {
    Ok(AccountAddress { dapp_id: array(bytes, offset)?, account_id: array(bytes, offset + 32)? })
}
fn amount_of(extra: CurrencyAmounts<'_>, currency: u32) -> u128 {
    extra.iter().find(|entry| entry.currency_id == currency).map_or(0, |entry| entry.amount)
}
fn expect_len(body: &[u8], len: usize) -> Result<()> {
    if body.len() == len {
        Ok(())
    } else {
        Err(ERR_ARGS)
    }
}
fn no_value(extra: CurrencyAmounts<'_>) -> Result<()> {
    if extra.is_empty() {
        Ok(())
    } else {
        Err(ERR_VALUE)
    }
}
fn not_paused(state: &State) -> Result<()> {
    if state.paused {
        Err(ERR_PAUSED)
    } else {
        Ok(())
    }
}
fn before_deadline(now: u32, deadline: u32) -> Result<()> {
    if now <= deadline {
        Ok(())
    } else {
        Err(ERR_DEADLINE)
    }
}
fn bump(state: &mut State) {
    state.ops = state.ops.saturating_add(1);
}

/// The vault must physically hold everything it keeps for holders, the pool
/// and the protocol, in every pool currency. The check holds whether
/// `input.extra` is the balance before or after this message's own transfer,
/// so it never depends on that detail.
fn check_solvency(state: &State, balance: CurrencyAmounts<'_>) -> Result<()> {
    for side in 0..2 {
        if amount_of(balance, state.config.currency[side]) < state.held(side)? {
            return Err(ERR_INSOLVENT);
        }
    }
    Ok(())
}

/// Folds the elapsed time at the pre-trade price into the TWAP accumulators.
fn accrue(state: &mut State, now: u32) {
    if !state.live() || now <= state.last_ts {
        return;
    }
    let elapsed = u128::from(now - state.last_ts);
    let q64 = 1u128 << 64;
    let price0 = math::mul_div_floor(state.reserve[1], q64, state.reserve[0]).unwrap_or(u128::MAX);
    let price1 = math::mul_div_floor(state.reserve[0], q64, state.reserve[1]).unwrap_or(u128::MAX);
    state.cumulative[0] = state.cumulative[0].wrapping_add(price0.wrapping_mul(elapsed));
    state.cumulative[1] = state.cumulative[1].wrapping_add(price1.wrapping_mul(elapsed));
    state.last_ts = now;
}

fn price_error(error: math::PriceError) -> u32 {
    match error {
        math::PriceError::InvalidInput => ERR_ARGS,
        math::PriceError::InsufficientLiquidity => ERR_NO_LIQUIDITY,
        math::PriceError::Overflow => ERR_OVERFLOW,
    }
}

// ------------------------------------------------------------------ init ---

fn initialize(input: MessageInput<'_>) -> Result<MessageOutput> {
    let deploy = input.deploy().map_err(|_| ERR_ARGS)?;
    let config = Config::parse(deploy.init_data()).ok_or(ERR_ARGS)?;
    if input.caller != Caller::External(config.admin) {
        return Err(ERR_AUTH);
    }
    if !input.state.is_empty() {
        return Err(ERR_ARGS);
    }
    no_value(input.message.extra())?;
    let event = Ev::new("amm.created")
        .topic(config.admin)
        .key("admin", config.admin)
        .u64("currency0", u64::from(config.currency[0]))
        .u64("currency1", u64::from(config.currency[1]))
        .u64("fee_bps", u64::from(config.swap_fee_bps))
        .u64("protocol_bps", u64::from(config.protocol_fee_bps));
    finish(&State::new(config), event)
}

// --------------------------------------------------------------- process ---

fn process(input: MessageInput<'_>) -> Result<MessageOutput> {
    let mut state = State::decode(input.state)?;
    if input.message.is_callback() {
        return callback(&input, state);
    }
    let Caller::External(actor) = input.caller else {
        return Err(ERR_AUTH);
    };
    check_solvency(&state, input.extra)?;
    let now = input.context.block_time;
    let body = input.message.body();
    let value = input.message.extra();
    match input.message.method_id() {
        ids::STATUS => {
            expect_len(body, 0)?;
            no_value(value)?;
            let event = Ev::new("amm.status")
                .topic(state.config.admin)
                .u128("fees0", state.fees[0])
                .u128("fees1", state.fees[1])
                .u128("lp_total", state.lp_total)
                .u64("ops", state.ops)
                .u64("paused", u64::from(state.paused))
                .u128("reserve0", state.reserve[0])
                .u128("reserve1", state.reserve[1]);
            finish(&state, event)
        }
        ids::TOPUP => {
            expect_len(body, 0)?;
            let mut shell = 0u128;
            for entry in value.iter() {
                if entry.currency_id != SHELL_CURRENCY_ID || entry.amount == 0 {
                    return Err(ERR_VALUE);
                }
                shell = entry.amount;
            }
            if shell == 0 {
                return Err(ERR_VALUE);
            }
            finish(
                &state,
                Ev::new("amm.fees_added").topic(state.config.admin).u128("amount", shell),
            )
        }
        ids::DEPOSIT => {
            expect_len(body, 0)?;
            let event = deposit(&mut state, actor, value)?;
            finish(&state, event)
        }
        ids::ADD_LIQUIDITY => {
            no_value(value)?;
            let event = add_liquidity(&mut state, actor, body, now)?;
            finish(&state, event)
        }
        ids::REMOVE_LIQUIDITY => {
            no_value(value)?;
            let event = remove_liquidity(&mut state, actor, body, now)?;
            finish(&state, event)
        }
        ids::SWAP_EXACT_IN => {
            no_value(value)?;
            let event = swap(&mut state, actor, body, now)?;
            finish(&state, event)
        }
        ids::WITHDRAW => {
            no_value(value)?;
            withdraw(state, actor, body, &input)
        }
        ids::COLLECT_FEES => {
            expect_len(body, 0)?;
            no_value(value)?;
            let event = collect_fees(&mut state, actor)?;
            finish(&state, event)
        }
        ids::SET_PARAMS => {
            no_value(value)?;
            let event = set_params(&mut state, actor, body)?;
            finish(&state, event)
        }
        _ => Err(ERR_ARGS),
    }
}

fn deposit(state: &mut State, actor: [u8; 32], value: CurrencyAmounts<'_>) -> Result<Ev> {
    not_paused(state)?;
    let mut credit = [0u128; 2];
    for entry in value.iter() {
        let side = state.side_of(entry.currency_id).ok_or(ERR_VALUE)?;
        if entry.amount == 0 || credit[side] != 0 {
            return Err(ERR_VALUE);
        }
        credit[side] = entry.amount;
    }
    if credit == [0; 2] {
        return Err(ERR_VALUE);
    }
    let account = state.account_mut_or_new(actor)?;
    for (balance, added) in account.balance.iter_mut().zip(credit) {
        *balance = balance.checked_add(added).ok_or(ERR_OVERFLOW)?;
    }
    bump(state);
    Ok(Ev::new("amm.deposited")
        .topic(actor)
        .u128("amount0", credit[0])
        .u128("amount1", credit[1])
        .key("holder", actor))
}

fn add_liquidity(state: &mut State, actor: [u8; 32], body: &[u8], now: u32) -> Result<Ev> {
    not_paused(state)?;
    expect_len(body, ADD_LIQUIDITY_ARGS_BYTES)?;
    let (desired0, desired1) = (read_u128(body, 0)?, read_u128(body, 16)?);
    let (min0, min1) = (read_u128(body, 32)?, read_u128(body, 48)?);
    before_deadline(now, read_u32(body, 64)?)?;
    if desired0 == 0 || desired1 == 0 {
        return Err(ERR_ARGS);
    }
    accrue(state, now);
    let first = !state.live();
    let (amount0, amount1, shares) = if first {
        let root = math::sqrt_product(desired0, desired1);
        let shares = root.checked_sub(MINIMUM_LIQUIDITY).filter(|s| *s != 0).ok_or(ERR_ZERO)?;
        (desired0, desired1, shares)
    } else {
        let [reserve0, reserve1] = state.reserve;
        let optimal1 = math::mul_div_floor(desired0, reserve1, reserve0).ok_or(ERR_OVERFLOW)?;
        let (amount0, amount1) = if optimal1 <= desired1 {
            (desired0, optimal1)
        } else {
            let optimal0 = math::mul_div_floor(desired1, reserve0, reserve1).ok_or(ERR_OVERFLOW)?;
            if optimal0 > desired0 {
                return Err(ERR_INVARIANT);
            }
            (optimal0, desired1)
        };
        if amount0 == 0 || amount1 == 0 {
            return Err(ERR_ZERO);
        }
        let shares0 = math::mul_div_floor(amount0, state.lp_total, reserve0).ok_or(ERR_OVERFLOW)?;
        let shares1 = math::mul_div_floor(amount1, state.lp_total, reserve1).ok_or(ERR_OVERFLOW)?;
        let shares = shares0.min(shares1);
        if shares == 0 {
            return Err(ERR_ZERO);
        }
        (amount0, amount1, shares)
    };
    if amount0 < min0 || amount1 < min1 {
        return Err(ERR_SLIPPAGE);
    }
    let index = state.account_index(actor).ok_or(ERR_BALANCE)?;
    let account = &mut state.accounts[index];
    account.balance[0] = account.balance[0].checked_sub(amount0).ok_or(ERR_BALANCE)?;
    account.balance[1] = account.balance[1].checked_sub(amount1).ok_or(ERR_BALANCE)?;
    account.lp = account.lp.checked_add(shares).ok_or(ERR_OVERFLOW)?;
    state.reserve[0] = state.reserve[0].checked_add(amount0).ok_or(ERR_OVERFLOW)?;
    state.reserve[1] = state.reserve[1].checked_add(amount1).ok_or(ERR_OVERFLOW)?;
    let minted = if first { shares.checked_add(MINIMUM_LIQUIDITY) } else { Some(shares) };
    state.lp_total = state.lp_total.checked_add(minted.ok_or(ERR_OVERFLOW)?).ok_or(ERR_OVERFLOW)?;
    if first {
        state.last_ts = now;
    }
    bump(state);
    Ok(Ev::new("amm.liquidity_added")
        .topic(actor)
        .u128("amount0", amount0)
        .u128("amount1", amount1)
        .key("holder", actor)
        .u128("lp_total", state.lp_total)
        .u128("shares", shares))
}

fn remove_liquidity(state: &mut State, actor: [u8; 32], body: &[u8], now: u32) -> Result<Ev> {
    expect_len(body, REMOVE_LIQUIDITY_ARGS_BYTES)?;
    let shares = read_u128(body, 0)?;
    let (min0, min1) = (read_u128(body, 16)?, read_u128(body, 32)?);
    before_deadline(now, read_u32(body, 48)?)?;
    if shares == 0 {
        return Err(ERR_ARGS);
    }
    if !state.live() {
        return Err(ERR_NO_LIQUIDITY);
    }
    let index = state.account_index(actor).ok_or(ERR_BALANCE)?;
    if state.accounts[index].lp < shares {
        return Err(ERR_BALANCE);
    }
    accrue(state, now);
    let amount0 =
        math::mul_div_floor(shares, state.reserve[0], state.lp_total).ok_or(ERR_OVERFLOW)?;
    let amount1 =
        math::mul_div_floor(shares, state.reserve[1], state.lp_total).ok_or(ERR_OVERFLOW)?;
    if amount0 == 0 || amount1 == 0 {
        return Err(ERR_ZERO);
    }
    if amount0 < min0 || amount1 < min1 {
        return Err(ERR_SLIPPAGE);
    }
    let account = &mut state.accounts[index];
    account.lp -= shares;
    account.balance[0] = account.balance[0].checked_add(amount0).ok_or(ERR_OVERFLOW)?;
    account.balance[1] = account.balance[1].checked_add(amount1).ok_or(ERR_OVERFLOW)?;
    state.reserve[0] -= amount0;
    state.reserve[1] -= amount1;
    state.lp_total -= shares;
    state.compact();
    bump(state);
    Ok(Ev::new("amm.liquidity_removed")
        .topic(actor)
        .u128("amount0", amount0)
        .u128("amount1", amount1)
        .key("holder", actor)
        .u128("lp_total", state.lp_total)
        .u128("shares", shares))
}

/// Exact-input swap. (`math::swap_in` prices the exact-output direction; the
/// contract does not expose it, to keep the guest under the deploy-body limit.)
fn swap(state: &mut State, actor: [u8; 32], body: &[u8], now: u32) -> Result<Ev> {
    not_paused(state)?;
    expect_len(body, SWAP_ARGS_BYTES)?;
    let direction = body[0];
    if direction > 1 {
        return Err(ERR_ARGS);
    }
    let (amount_in, min_out) = (read_u128(body, 1)?, read_u128(body, 17)?);
    before_deadline(now, read_u32(body, 33)?)?;
    if !state.live() {
        return Err(ERR_NO_LIQUIDITY);
    }
    let (side_in, side_out) = (usize::from(direction), 1 - usize::from(direction));
    let (reserve_in, reserve_out) = (state.reserve[side_in], state.reserve[side_out]);
    let fee = state.config.swap_fee_bps;
    let amount_out =
        math::swap_out(reserve_in, reserve_out, amount_in, fee).map_err(price_error)?;
    if amount_out == 0 {
        return Err(ERR_ZERO);
    }
    if amount_out < min_out {
        return Err(ERR_SLIPPAGE);
    }
    // Split the fee: the protocol's cut leaves the pool, the rest stays in the
    // reserve and so raises the value of every LP share.
    let after_fee = math::amount_after_fee(amount_in, fee).ok_or(ERR_OVERFLOW)?;
    let fee_total = amount_in - after_fee;
    let protocol_cut =
        math::mul_div_floor(fee_total, u128::from(state.config.protocol_fee_bps), math::BPS)
            .ok_or(ERR_OVERFLOW)?;
    let new_reserve_in = reserve_in.checked_add(amount_in - protocol_cut).ok_or(ERR_OVERFLOW)?;
    let new_reserve_out = reserve_out.checked_sub(amount_out).ok_or(ERR_NO_LIQUIDITY)?;
    if new_reserve_out == 0
        || math::cmp_products(new_reserve_in, new_reserve_out, reserve_in, reserve_out).is_lt()
    {
        return Err(ERR_INVARIANT);
    }
    let index = state.account_index(actor).ok_or(ERR_BALANCE)?;
    let account = &mut state.accounts[index];
    account.balance[side_in] =
        account.balance[side_in].checked_sub(amount_in).ok_or(ERR_BALANCE)?;
    account.balance[side_out] =
        account.balance[side_out].checked_add(amount_out).ok_or(ERR_OVERFLOW)?;
    accrue(state, now);
    state.reserve[side_in] = new_reserve_in;
    state.reserve[side_out] = new_reserve_out;
    state.fees[side_in] = state.fees[side_in].checked_add(protocol_cut).ok_or(ERR_OVERFLOW)?;
    state.compact();
    bump(state);
    Ok(Ev::new("amm.swapped")
        .topic(actor)
        .u128("amount_in", amount_in)
        .u128("amount_out", amount_out)
        .u64("direction", u64::from(direction))
        .key("holder", actor)
        .u128("protocol_fee", protocol_cut)
        .u128("reserve0", state.reserve[0])
        .u128("reserve1", state.reserve[1]))
}

/// Debits a holder and sends the funds to `recipient`, tracked until the
/// receiver's callback settles or refunds the payout.
fn withdraw(
    mut state: State,
    actor: [u8; 32],
    body: &[u8],
    input: &MessageInput<'_>,
) -> Result<MessageOutput> {
    expect_len(body, WITHDRAW_ARGS_BYTES)?;
    let currency = read_u32(body, 0)?;
    let amount = read_u128(body, 4)?;
    let recipient = read_address(body, 20)?;
    let side = state.side_of(currency).ok_or(ERR_ARGS)?;
    if amount == 0
        || recipient.dapp_id == [0; 32]
        || recipient.account_id == [0; 32]
        || recipient == input.destination
    {
        return Err(ERR_ARGS);
    }
    if state.pending.len() >= PENDING_SLOTS {
        return Err(ERR_CAPACITY);
    }
    if state.pending.iter().any(|entry| entry.recipient == recipient) {
        return Err(ERR_PENDING);
    }
    let fuel_reserve = state.config.fuel_reserve;
    if amount_of(input.extra, SHELL_CURRENCY_ID) < fuel_reserve {
        return Err(ERR_INSOLVENT);
    }
    let index = state.account_index(actor).ok_or(ERR_BALANCE)?;
    let account = &mut state.accounts[index];
    account.balance[side] = account.balance[side].checked_sub(amount).ok_or(ERR_BALANCE)?;
    let id = state.withdraw_seq;
    state.withdraw_seq = id.checked_add(1).ok_or(ERR_OVERFLOW)?;
    state.pending.push(Pending { recipient, holder: actor, currency, amount, id });
    state.compact();
    bump(&mut state);
    let event = Ev::new("amm.withdraw_pending")
        .topic(actor)
        .u128("amount", amount)
        .u64("currency", u64::from(currency))
        .key("holder", actor)
        .u64("id", id);
    let mut output = finish(&state, event)?;
    output.outbound_intents.push(OutboundIntent {
        input_index: 0,
        destination: recipient,
        fuel_fee_shells: fuel_reserve,
        extra: vec![CurrencyAmount { currency_id: currency, amount }],
        method_id: ids::RECEIVER_RECEIVE,
        body: payout_receipt(currency, id, amount),
    });
    output
        .callback_intents
        .push(CallbackIntent { outbound_index: 0, handler_method_id: ids::CALLBACK });
    Ok(output)
}

/// Settles or refunds the withdrawal addressed to the callback's source.
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
    } else {
        // Bounced: never release the slot without the exact principal.
        if returned != entry.amount || !message.body.is_empty() {
            return Err(ERR_CALLBACK);
        }
        let side = state.side_of(entry.currency).ok_or(ERR_STATE)?;
        let index = state.account_index(entry.holder).ok_or(ERR_STATE)?;
        let account = &mut state.accounts[index];
        account.balance[side] =
            account.balance[side].checked_add(entry.amount).ok_or(ERR_OVERFLOW)?;
    }
    state.pending.remove(position);
    state.push_receipt(header.cause_msg_hash);
    state.compact();
    bump(&mut state);
    let name = if success { "amm.withdraw_settled" } else { "amm.withdraw_failed" };
    let event = Ev::new(name)
        .topic(entry.holder)
        .u128("amount", entry.amount)
        .u64("currency", u64::from(entry.currency))
        .key("holder", entry.holder)
        .u64("id", entry.id);
    finish(&state, event)
}

fn require_admin(state: &State, actor: [u8; 32]) -> Result<()> {
    if actor == state.config.admin {
        Ok(())
    } else {
        Err(ERR_AUTH)
    }
}

/// Moves accrued protocol fees into the admin's ledger balance.
fn collect_fees(state: &mut State, actor: [u8; 32]) -> Result<Ev> {
    require_admin(state, actor)?;
    let fees = state.fees;
    if fees == [0; 2] {
        return Err(ERR_ZERO);
    }
    let account = state.account_mut_or_new(actor)?;
    for (balance, added) in account.balance.iter_mut().zip(fees) {
        *balance = balance.checked_add(added).ok_or(ERR_OVERFLOW)?;
    }
    state.fees = [0; 2];
    bump(state);
    Ok(Ev::new("amm.fees_collected")
        .topic(actor)
        .u128("amount0", fees[0])
        .u128("amount1", fees[1])
        .key("holder", actor))
}

/// Adjusts fees and the pause switch. Holders can always remove liquidity and
/// withdraw, even while paused, and no parameter touches balances or reserves.
fn set_params(state: &mut State, actor: [u8; 32], body: &[u8]) -> Result<Ev> {
    require_admin(state, actor)?;
    expect_len(body, SET_PARAMS_ARGS_BYTES)?;
    let swap_fee_bps = u16::from_le_bytes(array(body, 0)?);
    let protocol_fee_bps = u16::from_le_bytes(array(body, 2)?);
    if body[4] > 1 || swap_fee_bps > MAX_SWAP_FEE_BPS || protocol_fee_bps > MAX_PROTOCOL_FEE_BPS {
        return Err(ERR_ARGS);
    }
    state.config.swap_fee_bps = swap_fee_bps;
    state.config.protocol_fee_bps = protocol_fee_bps;
    state.paused = body[4] == 1;
    bump(state);
    Ok(Ev::new("amm.params_changed")
        .topic(actor)
        .u64("fee_bps", u64::from(swap_fee_bps))
        .u64("paused", u64::from(state.paused))
        .u64("protocol_bps", u64::from(protocol_fee_bps)))
}

#[cfg(all(target_arch = "wasm32", feature = "guest"))]
avm_contract_sdk::export_message_contract!(Amm, arena_bytes = 262_144);
