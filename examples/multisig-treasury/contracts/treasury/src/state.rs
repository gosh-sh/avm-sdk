//! Fixed-layout state, canonical decoding and the invariants every call relies on.

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
pub const KIND_SET_POLICY: u8 = 4;
pub const KIND_UNFREEZE: u8 = 5;

/// Proposal lifecycle. Executed, cancelled and expired proposals leave the table.
pub const STATUS_OPEN: u8 = 1;
pub const STATUS_QUEUED: u8 = 2;
pub const STATUS_EXECUTING: u8 = 3;

/// `proposer` value of a proposal whose proposer has since been removed.
pub const NO_PROPOSER: u8 = 0xff;

const BALANCE_BYTES: usize = 32;
const POLICY_BYTES: usize = 32;
const PROPOSAL_BYTES: usize = 112;
const PENDING_BYTES: usize = 96;

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
fn read_u128(bytes: &[u8], offset: usize) -> Result<u128> {
    array(bytes, offset).map(u128::from_le_bytes)
}
/// Sequential reader for a buffer whose length was checked once up front.
struct Rd<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl Rd<'_> {
    /// Little-endian integer of `len` bytes.
    #[inline(never)]
    fn num(&mut self, len: usize) -> u128 {
        let mut value = 0;
        for (index, byte) in self.bytes[self.at..self.at + len].iter().enumerate() {
            value |= u128::from(*byte) << (8 * index);
        }
        self.at += len;
        value
    }
    #[inline(never)]
    fn fill(&mut self, out: &mut [u8]) {
        out.copy_from_slice(&self.bytes[self.at..self.at + out.len()]);
        self.at += out.len();
    }
}
/// Sequential writer into a zero-filled buffer; skipped bytes stay zero.
struct Wr {
    bytes: Vec<u8>,
    at: usize,
}
impl Wr {
    #[inline(never)]
    fn num(&mut self, value: u128, len: usize) {
        self.put(&value.to_le_bytes()[..len]);
    }
    #[inline(never)]
    fn put(&mut self, value: &[u8]) {
        self.bytes[self.at..self.at + value.len()].copy_from_slice(value);
        self.at += value.len();
    }
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

/// Per-currency spending rule: transfers up to `fast_limit` skip the timelock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Policy {
    pub currency: u32,
    pub fast_limit: u128,
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
    pub fn credit(&mut self, currency: u32, amount: u128) -> Result<()> {
        if amount == 0 {
            return Ok(());
        }
        if let Some(entry) = self.balances.iter_mut().find(|b| b.currency == currency) {
            entry.held = entry.held.checked_add(amount).ok_or(ERR_OVERFLOW)?;
            return Ok(());
        }
        if self.balances.len() >= MAX_BALANCES {
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

    /// Reads the fixed layout, then accepts the bytes only if re-encoding the
    /// result reproduces them exactly: padding, unused slots and the counters
    /// all have a single canonical form.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != STATE_BYTES {
            return Err(ERR_STATE);
        }
        let mut r = Rd { bytes, at: 0 };
        let threshold = r.num(1) as u8;
        let frozen = r.num(1) == 1;
        let balance_count = (r.num(1) as usize).min(MAX_BALANCES);
        let policy_count = (r.num(1) as usize).min(MAX_POLICIES);
        let (delay, grace, ttl) = (r.num(4) as u32, r.num(4) as u32, r.num(4) as u32);
        let fuel_reserve = r.num(16);
        let (next_id, receipt_pos, ops) = (r.num(8) as u64, r.num(8) as u64, r.num(8) as u64);
        let proposal_count = (r.num(1) as usize).min(MAX_PROPOSALS);
        let pending_count = (r.num(1) as usize).min(MAX_PENDING);
        r.at = OFF_OWNERS;
        let mut owners = [[0; 32]; MAX_OWNERS];
        for owner in owners.iter_mut() {
            r.fill(owner);
        }
        let mut receipts = [[0; 32]; RECEIPT_SLOTS];
        for receipt in receipts.iter_mut() {
            r.fill(receipt);
        }
        let mut balances = Vec::new();
        for _ in 0..balance_count {
            let currency = r.num(4) as u32;
            r.at += 12;
            balances.push(Balance { currency, held: r.num(16) });
        }
        r.at = OFF_POLICIES;
        let mut policies = Vec::new();
        for _ in 0..policy_count {
            let currency = r.num(4) as u32;
            r.at += 12;
            policies.push(Policy { currency, fast_limit: r.num(16) });
        }
        r.at = OFF_PROPOSALS;
        let mut proposals = Vec::new();
        for _ in 0..proposal_count {
            let mut proposal = Proposal {
                id: r.num(8) as u64,
                kind: r.num(1) as u8,
                status: r.num(1) as u8,
                proposer: r.num(1) as u8,
                approvals: r.num(1) as u8,
                created: r.num(4) as u32,
                eta: r.num(4) as u32,
                expires: r.num(4) as u32,
                currency: r.num(4) as u32,
                aux: r.num(4) as u32,
                amount: r.num(16),
                target: [0; 64],
            };
            r.fill(&mut proposal.target);
            proposals.push(proposal);
        }
        r.at = OFF_PENDING;
        let mut pending = Vec::new();
        for _ in 0..pending_count {
            let mut recipient = AccountAddress { dapp_id: [0; 32], account_id: [0; 32] };
            r.fill(&mut recipient.dapp_id);
            r.fill(&mut recipient.account_id);
            let id = r.num(8) as u64;
            let currency = r.num(4) as u32;
            r.at += 4;
            pending.push(Pending { recipient, id, currency, amount: r.num(16) });
        }
        let state = Self {
            threshold,
            frozen,
            delay,
            grace,
            ttl,
            fuel_reserve,
            next_id,
            receipt_pos,
            ops,
            receipts,
            owners,
            balances,
            policies,
            proposals,
            pending,
        };
        state.validate()?;
        if state.encode() != bytes {
            return Err(ERR_STATE);
        }
        Ok(state)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Wr { bytes: vec![0; STATE_BYTES], at: 0 };
        w.num(u128::from(self.threshold), 1);
        w.num(u128::from(self.frozen), 1);
        w.num(self.balances.len() as u128, 1);
        w.num(self.policies.len() as u128, 1);
        w.num(u128::from(self.delay), 4);
        w.num(u128::from(self.grace), 4);
        w.num(u128::from(self.ttl), 4);
        w.num(self.fuel_reserve, 16);
        w.num(u128::from(self.next_id), 8);
        w.num(u128::from(self.receipt_pos), 8);
        w.num(u128::from(self.ops), 8);
        w.num(self.proposals.len() as u128, 1);
        w.num(self.pending.len() as u128, 1);
        w.at = OFF_OWNERS;
        for owner in &self.owners {
            w.put(owner);
        }
        for receipt in &self.receipts {
            w.put(receipt);
        }
        for entry in self.balances.iter().take(MAX_BALANCES) {
            w.num(u128::from(entry.currency), 4);
            w.at += 12;
            w.num(entry.held, 16);
        }
        w.at = OFF_POLICIES;
        for entry in self.policies.iter().take(MAX_POLICIES) {
            w.num(u128::from(entry.currency), 4);
            w.at += 12;
            w.num(entry.fast_limit, 16);
        }
        w.at = OFF_PROPOSALS;
        for entry in self.proposals.iter().take(MAX_PROPOSALS) {
            w.num(u128::from(entry.id), 8);
            w.num(u128::from(entry.kind), 1);
            w.num(u128::from(entry.status), 1);
            w.num(u128::from(entry.proposer), 1);
            w.num(u128::from(entry.approvals), 1);
            w.num(u128::from(entry.created), 4);
            w.num(u128::from(entry.eta), 4);
            w.num(u128::from(entry.expires), 4);
            w.num(u128::from(entry.currency), 4);
            w.num(u128::from(entry.aux), 4);
            w.num(entry.amount, 16);
            w.put(&entry.target);
        }
        w.at = OFF_PENDING;
        for entry in self.pending.iter().take(MAX_PENDING) {
            w.put(&entry.recipient.dapp_id);
            w.put(&entry.recipient.account_id);
            w.num(u128::from(entry.id), 8);
            w.num(u128::from(entry.currency), 4);
            w.at += 4;
            w.num(entry.amount, 16);
        }
        w.bytes
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
                || entry.fast_limit == 0
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
