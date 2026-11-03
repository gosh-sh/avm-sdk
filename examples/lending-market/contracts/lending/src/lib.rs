#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(target_arch = "wasm32", deny(unsafe_code), no_std)]

//! Over-collateralised lending market for one collateral asset and one
//! borrowed asset, with a utilisation-based interest curve.
//!
//! * Suppliers lend the borrowed asset and hold *shares*; the share price rises
//!   as borrowers pay interest.
//! * Borrowers lock collateral and borrow up to `ltv` of its value. Debt grows
//!   with a global borrow index, so interest accrues for everybody at once.
//! * A position above the `liq` threshold can be liquidated by anyone, who
//!   repays at most half of the debt and receives collateral at a bonus.
//! * Debt left over once all collateral is seized is written off: protocol
//!   reserves absorb the loss first, then suppliers.
//! * Funds enter through `deposit`, live on an internal ledger and leave
//!   through acknowledged payouts that are refunded if the receiver rejects them.
//! * The price comes from an admin-fed oracle with a per-update move cap and a
//!   staleness limit; it is the one trusted input.

extern crate alloc;

use alloc::{vec, vec::Vec};

use avm_contract_sdk::{
    encode_event_fields, AccountAddress, CallbackIntent, Caller, CanonicalMessage, Contract,
    CurrencyAmount, CurrencyAmounts, EventField, EventIntent, EventKind, EventValue, MessageInput,
    MessageOutput, OutboundIntent, StateUpdate,
};

pub mod ids;
pub mod math;
#[cfg(all(target_arch = "wasm32", feature = "guest"))]
#[allow(unsafe_code)]
mod rt;
pub mod state;

use math::{mul_div_ceil, mul_div_floor, ONE};
pub use state::{Account, Config, Pending, State, ACCOUNT_SLOTS, PENDING_SLOTS, STATE_BYTES};

/// SHELL (ECC 2) pays for execution; it can be deposited as FUEL but never lent.
pub const SHELL_CURRENCY_ID: u32 = 2;

pub const ERR_ARGS: u32 = 5000;
pub const ERR_AUTH: u32 = 5001;
pub const ERR_STATE: u32 = 5002;
pub const ERR_PAUSED: u32 = 5003;
pub const ERR_VALUE: u32 = 5004;
pub const ERR_BALANCE: u32 = 5005;
pub const ERR_LIQUIDITY: u32 = 5006;
pub const ERR_UNHEALTHY: u32 = 5007;
pub const ERR_HEALTHY: u32 = 5008;
pub const ERR_STALE: u32 = 5009;
pub const ERR_PRICE_MOVE: u32 = 5010;
pub const ERR_CAPACITY: u32 = 5011;
pub const ERR_PENDING: u32 = 5012;
pub const ERR_CALLBACK: u32 = 5013;
pub const ERR_INSOLVENT: u32 = 5014;
pub const ERR_OVERFLOW: u32 = 5015;
pub const ERR_ZERO: u32 = 5016;

/// `lend.action` codes.
pub const ACT_DEPOSIT: u64 = 1;
pub const ACT_SUPPLY: u64 = 2;
pub const ACT_REDEEM: u64 = 3;
pub const ACT_ADD_COLLATERAL: u64 = 4;
pub const ACT_REMOVE_COLLATERAL: u64 = 5;
pub const ACT_BORROW: u64 = 6;
pub const ACT_REPAY: u64 = 7;
pub const ACT_LIQUIDATE: u64 = 8;
pub const ACT_WITHDRAW_PENDING: u64 = 9;
pub const ACT_WITHDRAW_SETTLED: u64 = 10;
pub const ACT_WITHDRAW_FAILED: u64 = 11;
pub const ACT_WRITE_OFF: u64 = 12;
pub const ACT_COLLECT: u64 = 13;
pub const ACT_TOPUP: u64 = 14;

pub type Result<T> = core::result::Result<T, u32>;

pub const WITHDRAW_ARGS_BYTES: usize = 84;
pub const LIQUIDATE_ARGS_BYTES: usize = 48;
pub const PARAMS_ARGS_BYTES: usize = 23;
const BPS: u128 = 10_000;

pub struct Lending;

impl Contract for Lending {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        initialize(input).unwrap_or_else(MessageOutput::failure)
    }

    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        process(input).unwrap_or_else(MessageOutput::failure)
    }
}

/// The 32-byte receipt sent with a payout and echoed by an accepting receiver:
/// `LND1 | currency u32 | withdrawal id u64 | amount u128` (little endian).
pub fn payout_receipt(currency: u32, id: u64, amount: u128) -> Vec<u8> {
    let mut body = Vec::from(*b"LND1");
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

/// A `lend.action` event: what happened, how much, one action-specific number,
/// who did it and, where there is one, the counterparty.
fn action(code: u64, holder: [u8; 32], other: [u8; 32], amount: u128, aux: u128) -> Ev {
    Ev::new("lend.action", holder)
        .u64("action", code)
        .u128("amount", amount)
        .u128("aux", aux)
        .key("holder", holder)
        .key("other", other)
}

/// Validates the next state and wraps it, with its events, into a success output.
fn finish(state: &mut State, events: Vec<Ev>) -> Result<MessageOutput> {
    state.compact();
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
fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    array(bytes, offset).map(u16::from_le_bytes)
}
fn amount_of(extra: CurrencyAmounts<'_>, currency: u32) -> u128 {
    extra.iter().find(|entry| entry.currency_id == currency).map_or(0, |entry| entry.amount)
}
fn nonzero_address(address: AccountAddress) -> bool {
    address.dapp_id != [0; 32] && address.account_id != [0; 32]
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
fn require_admin(state: &State, actor: [u8; 32]) -> Result<()> {
    if actor == state.config.admin {
        Ok(())
    } else {
        Err(ERR_AUTH)
    }
}
/// A nonzero amount in a single 16-byte argument.
fn amount_arg(body: &[u8]) -> Result<u128> {
    expect_len(body, 16)?;
    match read_u128(body, 0)? {
        0 => Err(ERR_ZERO),
        amount => Ok(amount),
    }
}
fn mul_div(a: u128, b: u128, c: u128) -> Result<u128> {
    mul_div_floor(a, b, c).ok_or(ERR_OVERFLOW)
}

/// The market must physically hold everything it believes it owes. The check
/// holds whether `input.extra` is the balance before or after this message's own
/// transfer, so it never depends on that detail. Native callbacks validate their
/// returned principal separately.
fn check_solvency(state: &State, balance: CurrencyAmounts<'_>) -> Result<()> {
    for side in 0..2 {
        if amount_of(balance, state.config.currency[side]) < state.held(side)? {
            return Err(ERR_INSOLVENT);
        }
    }
    Ok(())
}

/// Accrues interest up to `now`: grows the borrow index by the rate the current
/// utilisation implies and moves the protocol's share into the reserves.
fn accrue(state: &mut State, now: u32) -> Result<()> {
    if now <= state.last_accrual {
        return Ok(());
    }
    let dt = now - state.last_accrual;
    state.last_accrual = now;
    if state.total_scaled == 0 {
        return Ok(());
    }
    let before = state.total_debt()?;
    let total = state.cash.checked_add(before).ok_or(ERR_OVERFLOW)?;
    let util = mul_div(before, BPS, total)? as u64;
    let c = state.config;
    let rate =
        math::borrow_rate_bps(util, c.kink.into(), c.base.into(), c.slope1.into(), c.slope2.into());
    state.index = math::grow_index(state.index, rate, dt).ok_or(ERR_OVERFLOW)?;
    let interest = state.total_debt()?.saturating_sub(before);
    let cut = mul_div(interest, u128::from(c.reserve_factor), BPS)?;
    state.reserves = state.reserves.checked_add(cut).ok_or(ERR_OVERFLOW)?;
    Ok(())
}

/// The oracle price, if it has been set and is recent enough.
fn fresh_price(state: &State, now: u32) -> Result<u128> {
    if state.price == 0 || now.saturating_sub(state.price_ts) > state.config.max_age {
        Err(ERR_STALE)
    } else {
        Ok(state.price)
    }
}

/// Collateral value in borrowed units, scaled down by a basis-point factor.
fn limit(collateral: u128, price: u128, bps: u16) -> Result<u128> {
    mul_div(mul_div(collateral, price, ONE)?, u128::from(bps), BPS)
}

/// Lowers an account's debt by `pay` borrowed units and returns the cash to the
/// market. The scaled amount is rounded down, so the market never loses value.
fn reduce_debt(state: &mut State, index: usize, pay: u128) -> Result<()> {
    let account = state.accounts[index];
    let scaled = if pay >= state.debt_of(account.scaled)? {
        account.scaled
    } else {
        mul_div(pay, ONE, state.index)?.min(account.scaled)
    };
    if scaled == 0 {
        return Err(ERR_ZERO);
    }
    state.accounts[index].scaled -= scaled;
    state.total_scaled -= scaled;
    state.cash = state.cash.checked_add(pay).ok_or(ERR_OVERFLOW)?;
    Ok(())
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
    let event = Ev::new("lend.created", config.admin)
        .key("admin", config.admin)
        .u64("collateral", u64::from(config.currency[0]))
        .u64("debt", u64::from(config.currency[1]))
        .u64("liq", u64::from(config.liq))
        .u64("ltv", u64::from(config.ltv));
    finish(&mut State::new(config, input.context.block_time), vec![event])
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
    accrue(&mut state, now)?;
    let body = input.message.body();
    let value = input.message.extra();
    let method = input.message.method_id();
    if method == ids::DEPOSIT {
        expect_len(body, 0)?;
        let events = deposit(&mut state, actor, value)?;
        check_solvency(&state, input.extra)?;
        return finish(&mut state, events);
    }
    if method == ids::TOPUP {
        expect_len(body, 0)?;
        let shell = match value.iter().collect::<Vec<_>>()[..] {
            [entry] if entry.currency_id == SHELL_CURRENCY_ID && entry.amount != 0 => entry.amount,
            _ => return Err(ERR_VALUE),
        };
        return finish(&mut state, vec![action(ACT_TOPUP, actor, [0; 32], shell, 0)]);
    }
    no_value(value)?;
    let event = match method {
        ids::SUPPLY => supply(&mut state, actor, amount_arg(body)?)?,
        ids::REDEEM => redeem(&mut state, actor, amount_arg(body)?)?,
        ids::ADD_COLLATERAL => add_collateral(&mut state, actor, amount_arg(body)?)?,
        ids::REMOVE_COLLATERAL => remove_collateral(&mut state, actor, amount_arg(body)?, now)?,
        ids::BORROW => borrow(&mut state, actor, amount_arg(body)?, now)?,
        ids::REPAY => repay(&mut state, actor, amount_arg(body)?)?,
        ids::LIQUIDATE => liquidate(&mut state, actor, body, now)?,
        ids::WRITE_OFF => write_off(&mut state, actor, body)?,
        ids::WITHDRAW => return withdraw(state, actor, body, &input),
        ids::SET_PRICE => set_price(&mut state, actor, amount_arg(body)?, now)?,
        ids::SET_PARAMS => set_params(&mut state, actor, body)?,
        ids::COLLECT_RESERVES => {
            expect_len(body, 0)?;
            collect_reserves(&mut state, actor)?
        }
        _ => return Err(ERR_ARGS),
    };
    finish(&mut state, vec![event])
}

// ---------------------------------------------------------------- ledger ---

fn deposit(state: &mut State, actor: [u8; 32], value: CurrencyAmounts<'_>) -> Result<Vec<Ev>> {
    let mut events = Vec::new();
    for entry in value.iter() {
        let side = state.side_of(entry.currency_id).ok_or(ERR_VALUE)?;
        if entry.amount == 0 {
            return Err(ERR_VALUE);
        }
        let account = state.account_mut_or_new(actor)?;
        account.balance[side] =
            account.balance[side].checked_add(entry.amount).ok_or(ERR_OVERFLOW)?;
        events.push(action(ACT_DEPOSIT, actor, [0; 32], entry.amount, entry.currency_id.into()));
    }
    if events.is_empty() {
        return Err(ERR_VALUE);
    }
    Ok(events)
}

fn account_of(state: &State, actor: [u8; 32]) -> Result<usize> {
    state.account_index(actor).ok_or(ERR_BALANCE)
}

// --------------------------------------------------------------- lending ---

/// Lends idle borrowed asset and mints shares at the current share price.
fn supply(state: &mut State, actor: [u8; 32], amount: u128) -> Result<Ev> {
    not_paused(state)?;
    let index = account_of(state, actor)?;
    let value = state.supply_value()?;
    let minted = if state.total_shares == 0 {
        amount
    } else if value == 0 {
        return Err(ERR_INSOLVENT); // every share is worthless; nobody may join
    } else {
        mul_div(amount, state.total_shares, value)?
    };
    if minted == 0 {
        return Err(ERR_ZERO);
    }
    let account = &mut state.accounts[index];
    account.balance[1] = account.balance[1].checked_sub(amount).ok_or(ERR_BALANCE)?;
    account.shares = account.shares.checked_add(minted).ok_or(ERR_OVERFLOW)?;
    state.total_shares = state.total_shares.checked_add(minted).ok_or(ERR_OVERFLOW)?;
    state.cash = state.cash.checked_add(amount).ok_or(ERR_OVERFLOW)?;
    Ok(action(ACT_SUPPLY, actor, [0; 32], amount, minted))
}

/// Burns shares for their current value, as far as the market has cash.
fn redeem(state: &mut State, actor: [u8; 32], shares: u128) -> Result<Ev> {
    let index = account_of(state, actor)?;
    if shares > state.accounts[index].shares {
        return Err(ERR_BALANCE);
    }
    let assets = mul_div(shares, state.supply_value()?, state.total_shares)?;
    if assets == 0 {
        return Err(ERR_ZERO);
    }
    if assets > state.available() {
        return Err(ERR_LIQUIDITY);
    }
    let account = &mut state.accounts[index];
    account.shares -= shares;
    account.balance[1] = account.balance[1].checked_add(assets).ok_or(ERR_OVERFLOW)?;
    state.total_shares -= shares;
    state.cash -= assets;
    Ok(action(ACT_REDEEM, actor, [0; 32], shares, assets))
}

fn add_collateral(state: &mut State, actor: [u8; 32], amount: u128) -> Result<Ev> {
    not_paused(state)?;
    let index = account_of(state, actor)?;
    let account = &mut state.accounts[index];
    account.balance[0] = account.balance[0].checked_sub(amount).ok_or(ERR_BALANCE)?;
    account.collateral = account.collateral.checked_add(amount).ok_or(ERR_OVERFLOW)?;
    let locked = account.collateral;
    state.total_collateral = state.total_collateral.checked_add(amount).ok_or(ERR_OVERFLOW)?;
    Ok(action(ACT_ADD_COLLATERAL, actor, [0; 32], amount, locked))
}

/// Unlocks collateral. With debt outstanding the position must stay within `ltv`.
fn remove_collateral(state: &mut State, actor: [u8; 32], amount: u128, now: u32) -> Result<Ev> {
    let index = account_of(state, actor)?;
    let account = state.accounts[index];
    let left = account.collateral.checked_sub(amount).ok_or(ERR_BALANCE)?;
    if account.scaled != 0 {
        let price = fresh_price(state, now)?;
        if state.debt_of(account.scaled)? > limit(left, price, state.config.ltv)? {
            return Err(ERR_UNHEALTHY);
        }
    }
    let account = &mut state.accounts[index];
    account.collateral = left;
    account.balance[0] = account.balance[0].checked_add(amount).ok_or(ERR_OVERFLOW)?;
    state.total_collateral -= amount;
    Ok(action(ACT_REMOVE_COLLATERAL, actor, [0; 32], amount, left))
}

fn borrow(state: &mut State, actor: [u8; 32], amount: u128, now: u32) -> Result<Ev> {
    not_paused(state)?;
    let index = account_of(state, actor)?;
    let price = fresh_price(state, now)?;
    if amount > state.available() {
        return Err(ERR_LIQUIDITY);
    }
    // Rounded up: the recorded debt is never below what was lent.
    let scaled = mul_div_ceil(amount, ONE, state.index).ok_or(ERR_OVERFLOW)?;
    let account = &mut state.accounts[index];
    account.scaled = account.scaled.checked_add(scaled).ok_or(ERR_OVERFLOW)?;
    account.balance[1] = account.balance[1].checked_add(amount).ok_or(ERR_OVERFLOW)?;
    let (collateral, owed) = (account.collateral, account.scaled);
    state.total_scaled = state.total_scaled.checked_add(scaled).ok_or(ERR_OVERFLOW)?;
    state.cash -= amount;
    let debt = state.debt_of(owed)?;
    if debt > limit(collateral, price, state.config.ltv)? {
        return Err(ERR_UNHEALTHY);
    }
    Ok(action(ACT_BORROW, actor, [0; 32], amount, debt))
}

/// Repays up to `amount` of the caller's own debt from idle borrowed asset.
fn repay(state: &mut State, actor: [u8; 32], amount: u128) -> Result<Ev> {
    let index = account_of(state, actor)?;
    let debt = state.debt_of(state.accounts[index].scaled)?;
    if debt == 0 {
        return Err(ERR_ZERO);
    }
    let pay = amount.min(debt);
    let balance = &mut state.accounts[index].balance[1];
    *balance = balance.checked_sub(pay).ok_or(ERR_BALANCE)?;
    reduce_debt(state, index, pay)?;
    let left = state.debt_of(state.accounts[index].scaled)?;
    Ok(action(ACT_REPAY, actor, [0; 32], pay, left))
}

/// Repays part of an unhealthy borrower's debt and takes collateral at a bonus.
fn liquidate(state: &mut State, actor: [u8; 32], body: &[u8], now: u32) -> Result<Ev> {
    expect_len(body, LIQUIDATE_ARGS_BYTES)?;
    let borrower: [u8; 32] = array(body, 0)?;
    let wanted = read_u128(body, 32)?;
    let target = state.account_index(borrower).ok_or(ERR_ARGS)?;
    let payer = account_of(state, actor)?;
    if borrower == actor || wanted == 0 {
        return Err(ERR_ARGS);
    }
    let price = fresh_price(state, now)?;
    let account = state.accounts[target];
    let debt = state.debt_of(account.scaled)?;
    if debt == 0 || debt <= limit(account.collateral, price, state.config.liq)? {
        return Err(ERR_HEALTHY);
    }
    // Close factor 50%, rounded up so a one-unit debt can still be closed.
    let mut repaid = wanted.min(debt - (debt >> 1));
    let bonus = BPS + u128::from(state.config.bonus);
    let mut seized = mul_div(mul_div(repaid, bonus, BPS)?, ONE, price)?;
    if seized > account.collateral {
        // Not enough collateral for the bonus: take it all, charge only its worth.
        seized = account.collateral;
        repaid = mul_div(mul_div(seized, price, ONE)?, BPS, bonus)?;
    }
    if repaid == 0 || seized == 0 {
        return Err(ERR_ZERO);
    }
    let balance = &mut state.accounts[payer].balance[1];
    *balance = balance.checked_sub(repaid).ok_or(ERR_BALANCE)?;
    reduce_debt(state, target, repaid)?;
    state.accounts[target].collateral -= seized;
    state.total_collateral -= seized;
    let gain = &mut state.accounts[payer].balance[0];
    *gain = gain.checked_add(seized).ok_or(ERR_OVERFLOW)?;
    Ok(action(ACT_LIQUIDATE, actor, borrower, repaid, seized))
}

/// Forgives the debt of a borrower whose collateral is gone. Anyone may call it:
/// the reserves pay first, the suppliers share what is left.
fn write_off(state: &mut State, actor: [u8; 32], body: &[u8]) -> Result<Ev> {
    expect_len(body, 32)?;
    let borrower: [u8; 32] = array(body, 0)?;
    let target = state.account_index(borrower).ok_or(ERR_ARGS)?;
    let account = state.accounts[target];
    if account.scaled == 0 || account.collateral != 0 {
        return Err(ERR_HEALTHY);
    }
    let loss = state.debt_of(account.scaled)?;
    let absorbed = loss.min(state.reserves);
    state.reserves -= absorbed;
    state.total_scaled -= account.scaled;
    state.accounts[target].scaled = 0;
    Ok(action(ACT_WRITE_OFF, actor, borrower, loss, absorbed))
}

// ----------------------------------------------------------------- admin ---

fn set_price(state: &mut State, actor: [u8; 32], price: u128, now: u32) -> Result<Ev> {
    require_admin(state, actor)?;
    if state.price != 0 {
        let allowed = mul_div(state.price, state.config.max_move.into(), BPS)?;
        if state.price.abs_diff(price) > allowed {
            return Err(ERR_PRICE_MOVE);
        }
    }
    state.price = price;
    state.price_ts = now;
    Ok(Ev::new("lend.price", actor).u128("price", price).u64("ts", u64::from(now)))
}

/// Adjusts the rate curve, risk limits and the pause switch. Interest was
/// accrued at the old rates before this call, so nothing applies retroactively.
/// Holders can always repay, redeem, withdraw and be liquidated while paused.
fn set_params(state: &mut State, actor: [u8; 32], body: &[u8]) -> Result<Ev> {
    require_admin(state, actor)?;
    expect_len(body, PARAMS_ARGS_BYTES)?;
    let mut config = state.config;
    for (slot, field) in [
        &mut config.ltv,
        &mut config.liq,
        &mut config.bonus,
        &mut config.reserve_factor,
        &mut config.kink,
        &mut config.base,
        &mut config.slope1,
        &mut config.slope2,
        &mut config.max_move,
    ]
    .into_iter()
    .enumerate()
    {
        *field = read_u16(body, slot * 2)?;
    }
    config.max_age = u32::from_le_bytes(array(body, 18)?);
    if body[22] > 1 || !config.params_valid() {
        return Err(ERR_ARGS);
    }
    state.config = config;
    state.paused = body[22] == 1;
    Ok(Ev::new("lend.params", actor)
        .u64("kink", config.kink.into())
        .u64("liq", config.liq.into())
        .u64("ltv", config.ltv.into())
        .u64("paused", u64::from(state.paused)))
}

/// Moves collected protocol reserves into the admin's ledger balance.
fn collect_reserves(state: &mut State, actor: [u8; 32]) -> Result<Ev> {
    require_admin(state, actor)?;
    let amount = state.reserves.min(state.cash);
    if amount == 0 {
        return Err(ERR_ZERO);
    }
    let account = state.account_mut_or_new(actor)?;
    account.balance[1] = account.balance[1].checked_add(amount).ok_or(ERR_OVERFLOW)?;
    state.reserves -= amount;
    state.cash -= amount;
    Ok(action(ACT_COLLECT, actor, [0; 32], amount, 0))
}

// ------------------------------------------------------------ withdrawal ---

/// Debits a holder and sends the funds to `recipient`, tracked until the
/// receiver's callback settles or refunds the payout.
fn withdraw(
    mut state: State,
    actor: [u8; 32],
    body: &[u8],
    input: &MessageInput<'_>,
) -> Result<MessageOutput> {
    expect_len(body, WITHDRAW_ARGS_BYTES)?;
    let currency = u32::from_le_bytes(array(body, 0)?);
    let amount = read_u128(body, 4)?;
    let recipient = AccountAddress { dapp_id: array(body, 20)?, account_id: array(body, 52)? };
    let side = state.side_of(currency).ok_or(ERR_ARGS)?;
    if amount == 0 || !nonzero_address(recipient) || recipient == input.destination {
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
    let index = account_of(&state, actor)?;
    let account = &mut state.accounts[index];
    account.balance[side] = account.balance[side].checked_sub(amount).ok_or(ERR_BALANCE)?;
    let id = state.withdraw_seq;
    state.withdraw_seq = id.checked_add(1).ok_or(ERR_OVERFLOW)?;
    state.pending.push(Pending { recipient, holder: actor, currency, amount, id });
    let event = action(ACT_WITHDRAW_PENDING, actor, [0; 32], amount, id.into());
    let mut output = finish(&mut state, vec![event])?;
    output.outbound_intents.push(OutboundIntent {
        input_index: 0,
        destination: recipient,
        fuel_fee_shells: fuel_reserve,
        extra: vec![CurrencyAmount { currency_id: currency, amount }],
        method_id: ids::PAYEE_RECEIVE,
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
    let success = header.status_code == 0;
    if success {
        // The authenticated native status proves delivery. The receiver controls
        // callback_result_body, so a bad echo must not retain a global pending slot.
        // The runtime returns unused SHELL only; principal on success is invalid.
        if returned != 0 {
            return Err(ERR_CALLBACK);
        }
    } else {
        // Bounced: never release the slot without the exact principal.
        if returned != entry.amount || !message.body.is_empty() {
            return Err(ERR_CALLBACK);
        }
        let side = state.side_of(entry.currency).ok_or(ERR_STATE)?;
        let index = state.account_index(entry.holder).ok_or(ERR_STATE)?;
        let balance = &mut state.accounts[index].balance[side];
        *balance = balance.checked_add(entry.amount).ok_or(ERR_OVERFLOW)?;
    }
    state.pending.remove(position);
    state.push_receipt(header.cause_msg_hash);
    let code = if success { ACT_WITHDRAW_SETTLED } else { ACT_WITHDRAW_FAILED };
    let event = action(code, entry.holder, [0; 32], entry.amount, entry.id.into());
    finish(&mut state, vec![event])
}

#[cfg(all(target_arch = "wasm32", feature = "guest"))]
avm_contract_sdk::export_message_contract!(Lending, arena_bytes = 262_144);
