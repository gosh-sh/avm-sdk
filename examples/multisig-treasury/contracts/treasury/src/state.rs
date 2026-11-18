//! Fixed-layout state, canonical decoding and the invariants every call relies on.

use alloc::{vec, vec::Vec};

use avm_contract_sdk::AccountAddress;

use crate::{
    Result, ERR_BALANCE, ERR_CAPACITY, ERR_OVERFLOW, ERR_STATE, MAX_DELAY, SHELL_CURRENCY_ID,
};

pub const MAX_OWNERS: usize = 8;
pub const MAX_BALANCES: usize = 8;
pub const MAX_POLICIES: usize = 4;
pub const MAX_PROPOSALS: usize = 16;
pub const MAX_PENDING: usize = 4;
pub const RECEIPT_SLOTS: usize = 8;

/// Proposal kinds.
pub const KIND_TRANSFER: u8 = 1;
pub const KIND_ADD_OWNER: u8 = 2;
pub const KIND_REMOVE_OWNER: u8 = 3;
pub const KIND_SET_THRESHOLD: u8 = 4;
pub const KIND_SET_DELAY: u8 = 5;
pub const KIND_SET_POLICY: u8 = 6;
pub const KIND_UNFREEZE: u8 = 7;

/// Proposal lifecycle. Executed, cancelled and expired proposals leave the table.
pub const STATUS_OPEN: u8 = 1;
pub const STATUS_QUEUED: u8 = 2;
pub const STATUS_EXECUTING: u8 = 3;

/// `proposer` value of a proposal whose proposer has since been removed.
pub const NO_PROPOSER: u8 = 0xff;

const BALANCE_BYTES: usize = 32;
const POLICY_BYTES: usize = 64;
const PROPOSAL_BYTES: usize = 112;
const PENDING_BYTES: usize = 96;

const OFF_DELAY: usize = 4;
const OFF_GRACE: usize = 8;
const OFF_TTL: usize = 12;
const OFF_FUEL: usize = 16;
const OFF_NEXT_ID: usize = 32;
const OFF_RECEIPT_POS: usize = 40;
const OFF_OPS: usize = 48;
const OFF_OWNERS: usize = 64;
const OFF_RECEIPTS: usize = OFF_OWNERS + MAX_OWNERS * 32;
const OFF_BALANCES: usize = OFF_RECEIPTS + RECEIPT_SLOTS * 32;
const OFF_POLICIES: usize = OFF_BALANCES + MAX_BALANCES * BALANCE_BYTES;
const OFF_PROPOSALS: usize = OFF_POLICIES + MAX_POLICIES * POLICY_BYTES;
const OFF_PENDING: usize = OFF_PROPOSALS + MAX_PROPOSALS * PROPOSAL_BYTES;
pub const STATE_BYTES: usize = OFF_PENDING + MAX_PENDING * PENDING_BYTES;

fn array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N]> {
    bytes.get(offset..offset + N).and_then(|slice| slice.try_into().ok()).ok_or(ERR_STATE)
}
fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    array(bytes, offset).map(u32::from_le_bytes)
}
fn read_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    array(bytes, offset).map(u64::from_le_bytes)
}
fn read_u128(bytes: &[u8], offset: usize) -> Result<u128> {
    array(bytes, offset).map(u128::from_le_bytes)
}
fn put(bytes: &mut [u8], offset: usize, value: &[u8]) {
    bytes[offset..offset + value.len()].copy_from_slice(value);
}
fn is_zero(bytes: &[u8], offset: usize, len: usize) -> bool {
    bytes.get(offset..offset + len).is_some_and(|slice| slice.iter().all(|byte| *byte == 0))
}
pub fn nonzero(address: AccountAddress) -> bool {
    address.dapp_id != [0; 32] && address.account_id != [0; 32]
}

/// Held funds of one currency (funds the treasury owns and may spend).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Balance {
    pub currency: u32,
    pub held: u128,
}

/// Per-currency spending rules: transfers up to `fast_limit` skip the timelock,
/// and no more than `window_limit` may leave per `window_secs` window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Policy {
    pub currency: u32,
    pub window_secs: u32,
    pub fast_limit: u128,
    pub window_limit: u128,
    pub window_start: u32,
    pub spent: u128,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Proposal {
    pub id: u64,
    pub kind: u8,
    pub status: u8,
    /// Owner slot of the proposer, or [`NO_PROPOSER`].
    pub proposer: u8,
    /// Bit `i` is set when the owner in slot `i` approves.
    pub approvals: u8,
    pub created: u32,
    /// Earliest execution time; zero while the proposal is open.
    pub eta: u32,
    /// Last moment the proposal may still be approved (open) or executed (queued).
    pub expires: u32,
    pub currency: u32,
    pub aux: u32,
    pub amount: u128,
    pub target: [u8; 64],
}

/// A transfer whose delivery has not been acknowledged. The proposal id doubles
/// as the receipt id: it is unique for the lifetime of the contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pending {
    pub recipient: AccountAddress,
    pub id: u64,
    pub currency: u32,
    pub amount: u128,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct State {
    pub threshold: u8,
    pub frozen: bool,
    pub delay: u32,
    pub grace: u32,
    pub ttl: u32,
    pub fuel_reserve: u128,
    pub next_id: u64,
    pub receipt_pos: u64,
    pub ops: u64,
    pub receipts: [[u8; 32]; RECEIPT_SLOTS],
    /// Owner slots; an all-zero key marks an empty slot. Slots never move, so
    /// approval bits stay attached to the same owner.
    pub owners: [[u8; 32]; MAX_OWNERS],
    pub balances: Vec<Balance>,
    pub policies: Vec<Policy>,
    pub proposals: Vec<Proposal>,
    pub pending: Vec<Pending>,
}

/// Constructor parameters, parsed from the deploy init data.
pub struct Init {
    pub threshold: u8,
    pub delay: u32,
    pub grace: u32,
    pub ttl: u32,
    pub fuel_reserve: u128,
    pub owners: Vec<[u8; 32]>,
}

impl Init {
    /// `threshold u8 | owners u8 | delay u32 | grace u32 | ttl u32 | fuel u128 | owner keys`.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let count = usize::from(*bytes.get(1).ok_or(ERR_STATE)?);
        if count == 0 || count > MAX_OWNERS || bytes.len() != 30 + count * 32 {
            return Err(ERR_STATE);
        }
        let mut owners = Vec::new();
        for index in 0..count {
            let key: [u8; 32] = array(bytes, 30 + index * 32)?;
            if key == [0; 32] {
                return Err(ERR_STATE);
            }
            owners.push(key);
        }
        Ok(Self {
            threshold: bytes[0],
            delay: read_u32(bytes, 2)?,
            grace: read_u32(bytes, 6)?,
            ttl: read_u32(bytes, 10)?,
            fuel_reserve: read_u128(bytes, 14)?,
            owners,
        })
    }
}

impl State {
    pub fn new(init: &Init) -> Result<Self> {
        let mut owners = [[0; 32]; MAX_OWNERS];
        for (slot, key) in owners.iter_mut().zip(&init.owners) {
            *slot = *key;
        }
        let state = Self {
            threshold: init.threshold,
            frozen: false,
            delay: init.delay,
            grace: init.grace,
            ttl: init.ttl,
            fuel_reserve: init.fuel_reserve,
            next_id: 1,
            receipt_pos: 0,
            ops: 0,
            receipts: [[0; 32]; RECEIPT_SLOTS],
            owners,
            balances: Vec::new(),
            policies: Vec::new(),
            proposals: Vec::new(),
            pending: Vec::new(),
        };
        state.validate()?;
        Ok(state)
    }

    // ----------------------------------------------------------- owners ---

    pub fn owner_count(&self) -> usize {
        self.owners.iter().filter(|key| **key != [0; 32]).count()
    }
    pub fn owner_slot(&self, key: &[u8; 32]) -> Option<usize> {
        if *key == [0; 32] {
            return None;
        }
        self.owners.iter().position(|owner| owner == key)
    }
    /// Bitmask of occupied owner slots.
    pub fn owner_mask(&self) -> u8 {
        let mut mask = 0;
        for (slot, key) in self.owners.iter().enumerate() {
            if *key != [0; 32] {
                mask |= 1 << slot;
            }
        }
        mask
    }

    // --------------------------------------------------------- balances ---

    pub fn held(&self, currency: u32) -> u128 {
        self.balances.iter().find(|b| b.currency == currency).map_or(0, |b| b.held)
    }
    /// Reserve SHELL and every distinct currency that a pending payout may return.
    /// A held currency and several pending payouts in that currency share one slot.
    fn balance_slots(&self, additional: Option<u32>) -> usize {
        let mut currencies = [0; MAX_BALANCES + MAX_PENDING + 2];
        let mut count = 0;
        for currency in core::iter::once(SHELL_CURRENCY_ID)
            .chain(self.balances.iter().map(|entry| entry.currency))
            .chain(self.pending.iter().map(|entry| entry.currency))
            .chain(additional)
        {
            if !currencies[..count].contains(&currency) {
                currencies[count] = currency;
                count += 1;
            }
        }
        count
    }
    pub fn credit(&mut self, currency: u32, amount: u128) -> Result<()> {
        if amount == 0 {
            return Ok(());
        }
        if let Some(entry) = self.balances.iter_mut().find(|b| b.currency == currency) {
            entry.held = entry.held.checked_add(amount).ok_or(ERR_OVERFLOW)?;
            return Ok(());
        }
        if self.balance_slots(Some(currency)) > MAX_BALANCES {
            return Err(ERR_CAPACITY);
        }
        self.balances.push(Balance { currency, held: amount });
        Ok(())
    }
    pub fn debit(&mut self, currency: u32, amount: u128) -> Result<()> {
        let index = self.balances.iter().position(|b| b.currency == currency).ok_or(ERR_BALANCE)?;
        let held = self.balances[index].held.checked_sub(amount).ok_or(ERR_BALANCE)?;
        if held == 0 {
            self.balances.remove(index);
        } else {
            self.balances[index].held = held;
        }
        Ok(())
    }

    // ----------------------------------------------------------- tables ---

    pub fn policy(&self, currency: u32) -> Option<&Policy> {
        self.policies.iter().find(|p| p.currency == currency)
    }
    pub fn proposal_index(&self, id: u64) -> Option<usize> {
        self.proposals.iter().position(|p| p.id == id)
    }
    pub fn receipt_seen(&self, hash: &[u8; 32]) -> bool {
        self.receipts.contains(hash)
    }
    pub fn push_receipt(&mut self, hash: [u8; 32]) {
        self.receipts[(self.receipt_pos % RECEIPT_SLOTS as u64) as usize] = hash;
        self.receipt_pos += 1;
    }

    // ------------------------------------------------------- encoding ---

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != STATE_BYTES || !is_zero(bytes, 59, 5) {
            return Err(ERR_STATE);
        }
        let (frozen, balance_count, policy_count) = (bytes[1], bytes[2], bytes[3]);
        let (proposal_count, pending_count) = (bytes[56], bytes[57]);
        if frozen > 1
            || bytes[58] != 0
            || usize::from(balance_count) > MAX_BALANCES
            || usize::from(policy_count) > MAX_POLICIES
            || usize::from(proposal_count) > MAX_PROPOSALS
            || usize::from(pending_count) > MAX_PENDING
        {
            return Err(ERR_STATE);
        }
        let mut owners = [[0; 32]; MAX_OWNERS];
        for (index, owner) in owners.iter_mut().enumerate() {
            *owner = array(bytes, OFF_OWNERS + index * 32)?;
        }
        let mut receipts = [[0; 32]; RECEIPT_SLOTS];
        for (index, receipt) in receipts.iter_mut().enumerate() {
            *receipt = array(bytes, OFF_RECEIPTS + index * 32)?;
        }
        let mut balances = Vec::new();
        for index in 0..MAX_BALANCES {
            let base = OFF_BALANCES + index * BALANCE_BYTES;
            if index < usize::from(balance_count) {
                if !is_zero(bytes, base + 4, 12) {
                    return Err(ERR_STATE);
                }
                balances.push(Balance {
                    currency: read_u32(bytes, base)?,
                    held: read_u128(bytes, base + 16)?,
                });
            } else if !is_zero(bytes, base, BALANCE_BYTES) {
                return Err(ERR_STATE);
            }
        }
        let mut policies = Vec::new();
        for index in 0..MAX_POLICIES {
            let base = OFF_POLICIES + index * POLICY_BYTES;
            if index < usize::from(policy_count) {
                if !is_zero(bytes, base + 44, 4) {
                    return Err(ERR_STATE);
                }
                policies.push(Policy {
                    currency: read_u32(bytes, base)?,
                    window_secs: read_u32(bytes, base + 4)?,
                    fast_limit: read_u128(bytes, base + 8)?,
                    window_limit: read_u128(bytes, base + 24)?,
                    window_start: read_u32(bytes, base + 40)?,
                    spent: read_u128(bytes, base + 48)?,
                });
            } else if !is_zero(bytes, base, POLICY_BYTES) {
                return Err(ERR_STATE);
            }
        }
        let mut proposals = Vec::new();
        for index in 0..MAX_PROPOSALS {
            let base = OFF_PROPOSALS + index * PROPOSAL_BYTES;
            if index < usize::from(proposal_count) {
                proposals.push(Proposal {
                    id: read_u64(bytes, base)?,
                    kind: bytes[base + 8],
                    status: bytes[base + 9],
                    proposer: bytes[base + 10],
                    approvals: bytes[base + 11],
                    created: read_u32(bytes, base + 12)?,
                    eta: read_u32(bytes, base + 16)?,
                    expires: read_u32(bytes, base + 20)?,
                    currency: read_u32(bytes, base + 24)?,
                    aux: read_u32(bytes, base + 28)?,
                    amount: read_u128(bytes, base + 32)?,
                    target: array(bytes, base + 48)?,
                });
            } else if !is_zero(bytes, base, PROPOSAL_BYTES) {
                return Err(ERR_STATE);
            }
        }
        let mut pending = Vec::new();
        for index in 0..MAX_PENDING {
            let base = OFF_PENDING + index * PENDING_BYTES;
            if index < usize::from(pending_count) {
                if !is_zero(bytes, base + 76, 4) {
                    return Err(ERR_STATE);
                }
                pending.push(Pending {
                    recipient: AccountAddress {
                        dapp_id: array(bytes, base)?,
                        account_id: array(bytes, base + 32)?,
                    },
                    id: read_u64(bytes, base + 64)?,
                    currency: read_u32(bytes, base + 72)?,
                    amount: read_u128(bytes, base + 80)?,
                });
            } else if !is_zero(bytes, base, PENDING_BYTES) {
                return Err(ERR_STATE);
            }
        }
        let state = Self {
            threshold: bytes[0],
            frozen: frozen == 1,
            delay: read_u32(bytes, OFF_DELAY)?,
            grace: read_u32(bytes, OFF_GRACE)?,
            ttl: read_u32(bytes, OFF_TTL)?,
            fuel_reserve: read_u128(bytes, OFF_FUEL)?,
            next_id: read_u64(bytes, OFF_NEXT_ID)?,
            receipt_pos: read_u64(bytes, OFF_RECEIPT_POS)?,
            ops: read_u64(bytes, OFF_OPS)?,
            receipts,
            owners,
            balances,
            policies,
            proposals,
            pending,
        };
        state.validate()?;
        Ok(state)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = vec![0; STATE_BYTES];
        bytes[0] = self.threshold;
        bytes[1] = u8::from(self.frozen);
        bytes[2] = self.balances.len() as u8;
        bytes[3] = self.policies.len() as u8;
        put(&mut bytes, OFF_DELAY, &self.delay.to_le_bytes());
        put(&mut bytes, OFF_GRACE, &self.grace.to_le_bytes());
        put(&mut bytes, OFF_TTL, &self.ttl.to_le_bytes());
        put(&mut bytes, OFF_FUEL, &self.fuel_reserve.to_le_bytes());
        put(&mut bytes, OFF_NEXT_ID, &self.next_id.to_le_bytes());
        put(&mut bytes, OFF_RECEIPT_POS, &self.receipt_pos.to_le_bytes());
        put(&mut bytes, OFF_OPS, &self.ops.to_le_bytes());
        bytes[56] = self.proposals.len() as u8;
        bytes[57] = self.pending.len() as u8;
        for (index, owner) in self.owners.iter().enumerate() {
            put(&mut bytes, OFF_OWNERS + index * 32, owner);
        }
        for (index, receipt) in self.receipts.iter().enumerate() {
            put(&mut bytes, OFF_RECEIPTS + index * 32, receipt);
        }
        for (index, entry) in self.balances.iter().enumerate().take(MAX_BALANCES) {
            let base = OFF_BALANCES + index * BALANCE_BYTES;
            put(&mut bytes, base, &entry.currency.to_le_bytes());
            put(&mut bytes, base + 16, &entry.held.to_le_bytes());
        }
        for (index, entry) in self.policies.iter().enumerate().take(MAX_POLICIES) {
            let base = OFF_POLICIES + index * POLICY_BYTES;
            put(&mut bytes, base, &entry.currency.to_le_bytes());
            put(&mut bytes, base + 4, &entry.window_secs.to_le_bytes());
            put(&mut bytes, base + 8, &entry.fast_limit.to_le_bytes());
            put(&mut bytes, base + 24, &entry.window_limit.to_le_bytes());
            put(&mut bytes, base + 40, &entry.window_start.to_le_bytes());
            put(&mut bytes, base + 48, &entry.spent.to_le_bytes());
        }
        for (index, entry) in self.proposals.iter().enumerate().take(MAX_PROPOSALS) {
            let base = OFF_PROPOSALS + index * PROPOSAL_BYTES;
            put(&mut bytes, base, &entry.id.to_le_bytes());
            bytes[base + 8] = entry.kind;
            bytes[base + 9] = entry.status;
            bytes[base + 10] = entry.proposer;
            bytes[base + 11] = entry.approvals;
            put(&mut bytes, base + 12, &entry.created.to_le_bytes());
            put(&mut bytes, base + 16, &entry.eta.to_le_bytes());
            put(&mut bytes, base + 20, &entry.expires.to_le_bytes());
            put(&mut bytes, base + 24, &entry.currency.to_le_bytes());
            put(&mut bytes, base + 28, &entry.aux.to_le_bytes());
            put(&mut bytes, base + 32, &entry.amount.to_le_bytes());
            put(&mut bytes, base + 48, &entry.target);
        }
        for (index, entry) in self.pending.iter().enumerate().take(MAX_PENDING) {
            let base = OFF_PENDING + index * PENDING_BYTES;
            put(&mut bytes, base, &entry.recipient.dapp_id);
            put(&mut bytes, base + 32, &entry.recipient.account_id);
            put(&mut bytes, base + 64, &entry.id.to_le_bytes());
            put(&mut bytes, base + 72, &entry.currency.to_le_bytes());
            put(&mut bytes, base + 80, &entry.amount.to_le_bytes());
        }
        bytes
    }

    // ------------------------------------------------------- invariants ---

    /// Every structural and accounting rule the contract relies on. It runs on
    /// each decode and before each state output, so a bug cannot persist a
    /// state that a later call would not accept.
    pub fn validate(&self) -> Result<()> {
        let owners = self.owner_count();
        if self.threshold == 0
            || usize::from(self.threshold) > owners
            || self.delay > MAX_DELAY
            || self.grace == 0
            || self.ttl == 0
            || self.fuel_reserve == 0
            || self.balances.len() > MAX_BALANCES
            || self.policies.len() > MAX_POLICIES
            || self.proposals.len() > MAX_PROPOSALS
            || self.pending.len() > MAX_PENDING
        {
            return Err(ERR_STATE);
        }
        if self.balance_slots(None) > MAX_BALANCES {
            return Err(ERR_STATE);
        }
        for (index, key) in self.owners.iter().enumerate() {
            if *key != [0; 32] && self.owners[..index].contains(key) {
                return Err(ERR_STATE);
            }
        }
        for (index, entry) in self.balances.iter().enumerate() {
            if entry.held == 0
                || self.balances[..index].iter().any(|other| other.currency == entry.currency)
            {
                return Err(ERR_STATE);
            }
        }
        for (index, entry) in self.policies.iter().enumerate() {
            if entry.currency == SHELL_CURRENCY_ID
                || entry.window_secs == 0
                || entry.window_limit == 0
                || entry.fast_limit > entry.window_limit
                || entry.spent > entry.window_limit
                || self.policies[..index].iter().any(|other| other.currency == entry.currency)
            {
                return Err(ERR_STATE);
            }
        }
        let mask = self.owner_mask();
        for (index, entry) in self.proposals.iter().enumerate() {
            let executing = entry.status == STATUS_EXECUTING;
            let proposer_ok = entry.proposer == NO_PROPOSER
                || (usize::from(entry.proposer) < MAX_OWNERS && mask & (1 << entry.proposer) != 0);
            if !(KIND_TRANSFER..=KIND_UNFREEZE).contains(&entry.kind)
                || !(STATUS_OPEN..=STATUS_EXECUTING).contains(&entry.status)
                || !proposer_ok
                || entry.id == 0
                || entry.id >= self.next_id
                || (index > 0 && self.proposals[index - 1].id >= entry.id)
                || (!executing && entry.approvals & !mask != 0)
                || (entry.status == STATUS_OPEN && entry.eta != 0)
                || (entry.status == STATUS_QUEUED
                    && (entry.approvals.count_ones() < u32::from(self.threshold)
                        || entry.eta == 0
                        || entry.expires < entry.eta))
                || (executing && entry.kind != KIND_TRANSFER)
            {
                return Err(ERR_STATE);
            }
        }
        for (index, entry) in self.pending.iter().enumerate() {
            let proposal = self.proposal_index(entry.id).map(|i| &self.proposals[i]);
            let linked = proposal.is_some_and(|p| {
                p.status == STATUS_EXECUTING
                    && p.currency == entry.currency
                    && p.amount == entry.amount
                    && p.target[..32] == entry.recipient.dapp_id
                    && p.target[32..] == entry.recipient.account_id
            });
            if !nonzero(entry.recipient)
                || entry.amount == 0
                || entry.currency == SHELL_CURRENCY_ID
                || !linked
                || self.pending[..index].iter().any(|other| other.recipient == entry.recipient)
            {
                return Err(ERR_STATE);
            }
        }
        let executing = self.proposals.iter().filter(|p| p.status == STATUS_EXECUTING).count();
        if executing != self.pending.len() {
            return Err(ERR_STATE);
        }
        Ok(())
    }
}
