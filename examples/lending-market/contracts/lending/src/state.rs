//! Fixed-layout state, canonical decoding and the accounting invariants.

use alloc::{vec, vec::Vec};

use avm_contract_sdk::AccountAddress;

use crate::{
    math::{mul_div_ceil, ONE},
    Result, ERR_CAPACITY, ERR_OVERFLOW, ERR_STATE, SHELL_CURRENCY_ID,
};

pub const CONFIG_BYTES: usize = 80;
pub const ACCOUNT_SLOTS: usize = 24;
pub const PENDING_SLOTS: usize = 8;
pub const RECEIPT_SLOTS: usize = 16;

const ACCOUNT_BYTES: usize = 112;
const PENDING_BYTES: usize = 136;

const OFF_PAUSED: usize = 80;
const OFF_PRICE: usize = 88;
const OFF_PRICE_TS: usize = 104;
const OFF_CASH: usize = 112;
const OFF_RESERVES: usize = 128;
const OFF_SHARES: usize = 144;
const OFF_SCALED: usize = 160;
const OFF_INDEX: usize = 176;
const OFF_ACCRUAL: usize = 192;
const OFF_COLLATERAL: usize = 200;
const OFF_OPS: usize = 216;
const OFF_SEQ: usize = 224;
const OFF_RECEIPT_POS: usize = 232;
const OFF_RECEIPTS: usize = 240;
const OFF_ACCOUNTS: usize = OFF_RECEIPTS + RECEIPT_SLOTS * 32;
const OFF_PENDING: usize = OFF_ACCOUNTS + ACCOUNT_SLOTS * ACCOUNT_BYTES;
pub const STATE_BYTES: usize = OFF_PENDING + PENDING_SLOTS * PENDING_BYTES;

fn array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N]> {
    bytes.get(offset..offset + N).and_then(|slice| slice.try_into().ok()).ok_or(ERR_STATE)
}
fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    array(bytes, offset).map(u16::from_le_bytes)
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
fn nonzero(address: AccountAddress) -> bool {
    address.dapp_id != [0; 32] && address.account_id != [0; 32]
}

/// Market parameters. `admin`, the two currencies and `fuel_reserve` are
/// immutable; everything else can be changed by the admin within hard bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    pub admin: [u8; 32],
    /// `[collateral asset, borrowed asset]`.
    pub currency: [u32; 2],
    /// Maximum loan-to-value when borrowing or withdrawing collateral (bps).
    pub ltv: u16,
    /// Loan-to-value above which a position can be liquidated (bps).
    pub liq: u16,
    /// Extra collateral a liquidator receives (bps).
    pub bonus: u16,
    /// Share of borrower interest kept as protocol reserves (bps).
    pub reserve_factor: u16,
    /// Utilisation at which the rate curve steepens (bps).
    pub kink: u16,
    pub base: u16,
    pub slope1: u16,
    pub slope2: u16,
    /// Largest single price update, relative to the previous price (bps).
    pub max_move: u16,
    /// Seconds after which the price is too old to be used.
    pub max_age: u32,
    /// SHELL reserved as FUEL for each payout message.
    pub fuel_reserve: u128,
}

impl Config {
    /// Strictly parses and validates the 80-byte wire form.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != CONFIG_BYTES || bytes[58..60] != [0; 2] {
            return None;
        }
        let config = Self {
            admin: array(bytes, 0).ok()?,
            currency: [read_u32(bytes, 32).ok()?, read_u32(bytes, 36).ok()?],
            ltv: read_u16(bytes, 40).ok()?,
            liq: read_u16(bytes, 42).ok()?,
            bonus: read_u16(bytes, 44).ok()?,
            reserve_factor: read_u16(bytes, 46).ok()?,
            kink: read_u16(bytes, 48).ok()?,
            base: read_u16(bytes, 50).ok()?,
            slope1: read_u16(bytes, 52).ok()?,
            slope2: read_u16(bytes, 54).ok()?,
            max_move: read_u16(bytes, 56).ok()?,
            max_age: read_u32(bytes, 60).ok()?,
            fuel_reserve: read_u128(bytes, 64).ok()?,
        };
        config.is_valid().then_some(config)
    }

    pub fn is_valid(&self) -> bool {
        self.admin != [0; 32]
            && self.currency[0] != self.currency[1]
            && !self.currency.contains(&SHELL_CURRENCY_ID)
            && self.fuel_reserve != 0
            && self.params_valid()
    }

    /// The bounds of every admin-adjustable parameter. They guarantee that a
    /// liquidation at the threshold always leaves the position no worse off and
    /// that no rate or move can be set to an extreme value.
    pub fn params_valid(&self) -> bool {
        self.ltv != 0
            && self.ltv < self.liq
            && self.liq <= 9_500
            && self.bonus <= 2_000
            && u32::from(self.liq) * (10_000 + u32::from(self.bonus)) <= 99_000_000
            && self.reserve_factor <= 5_000
            && (1..=9_999).contains(&self.kink)
            && self.base <= 5_000
            && self.slope1 <= 5_000
            && self.slope2 <= 50_000
            && (1..=5_000).contains(&self.max_move)
            && self.max_age != 0
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = vec![0; CONFIG_BYTES];
        self.write_to(&mut bytes);
        bytes
    }

    /// Writes the wire form into the first `CONFIG_BYTES` of `bytes`.
    fn write_to(&self, bytes: &mut [u8]) {
        put(bytes, 0, &self.admin);
        put(bytes, 32, &self.currency[0].to_le_bytes());
        put(bytes, 36, &self.currency[1].to_le_bytes());
        for (index, value) in [
            self.ltv,
            self.liq,
            self.bonus,
            self.reserve_factor,
            self.kink,
            self.base,
            self.slope1,
            self.slope2,
            self.max_move,
        ]
        .into_iter()
        .enumerate()
        {
            put(bytes, 40 + index * 2, &value.to_le_bytes());
        }
        put(bytes, 60, &self.max_age.to_le_bytes());
        put(bytes, 64, &self.fuel_reserve.to_le_bytes());
    }
}

/// One participant. Balances are claimable internal funds; the rest is the
/// participant's position in the market.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Account {
    pub key: [u8; 32],
    /// Idle funds per currency, `[collateral asset, borrowed asset]`.
    pub balance: [u128; 2],
    /// Collateral locked in the market.
    pub collateral: u128,
    /// Supply shares of the borrowed asset.
    pub shares: u128,
    /// Debt in index-scaled units: `debt = ceil(scaled * index / ONE)`.
    pub scaled: u128,
}

impl Account {
    pub fn is_empty(&self) -> bool {
        self.balance == [0; 2] && self.collateral == 0 && self.shares == 0 && self.scaled == 0
    }
}

/// A withdrawal whose delivery has not been acknowledged yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pending {
    pub recipient: AccountAddress,
    pub holder: [u8; 32],
    pub currency: u32,
    pub amount: u128,
    pub id: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct State {
    pub config: Config,
    pub paused: bool,
    /// Price of one collateral unit in borrowed units, scaled by `ONE`; zero until set.
    pub price: u128,
    pub price_ts: u32,
    /// Borrowed asset physically in the market, available to lend out.
    pub cash: u128,
    /// Protocol share of interest, a claim inside `cash + debt`.
    pub reserves: u128,
    pub total_shares: u128,
    pub total_scaled: u128,
    pub index: u128,
    pub last_accrual: u32,
    pub total_collateral: u128,
    pub ops: u64,
    pub withdraw_seq: u64,
    pub receipt_pos: u64,
    pub receipts: [[u8; 32]; RECEIPT_SLOTS],
    pub accounts: Vec<Account>,
    pub pending: Vec<Pending>,
}

impl State {
    pub fn new(config: Config, now: u32) -> Self {
        Self {
            config,
            paused: false,
            price: 0,
            price_ts: 0,
            cash: 0,
            reserves: 0,
            total_shares: 0,
            total_scaled: 0,
            index: ONE,
            last_accrual: now,
            total_collateral: 0,
            ops: 0,
            withdraw_seq: 0,
            receipt_pos: 0,
            receipts: [[0; 32]; RECEIPT_SLOTS],
            accounts: Vec::new(),
            pending: Vec::new(),
        }
    }

    pub fn side_of(&self, currency: u32) -> Option<usize> {
        self.config.currency.iter().position(|id| *id == currency)
    }

    pub fn account_index(&self, key: [u8; 32]) -> Option<usize> {
        self.accounts.iter().position(|account| account.key == key)
    }

    pub fn account_mut_or_new(&mut self, key: [u8; 32]) -> Result<&mut Account> {
        let index = match self.account_index(key) {
            Some(index) => index,
            None => {
                if self.accounts.len() >= ACCOUNT_SLOTS {
                    return Err(ERR_CAPACITY);
                }
                self.accounts.push(Account {
                    key,
                    balance: [0; 2],
                    collateral: 0,
                    shares: 0,
                    scaled: 0,
                });
                self.accounts.len() - 1
            }
        };
        Ok(&mut self.accounts[index])
    }

    /// Drops empty accounts that no pending withdrawal can still refund into.
    pub fn compact(&mut self) {
        let mut index = 0;
        while index < self.accounts.len() {
            let account = &self.accounts[index];
            if account.is_empty() && !self.pending.iter().any(|entry| entry.holder == account.key) {
                self.accounts.remove(index);
            } else {
                index += 1;
            }
        }
    }

    /// Debt in borrowed units for an index-scaled amount, rounded up.
    pub fn debt_of(&self, scaled: u128) -> Result<u128> {
        mul_div_ceil(scaled, self.index, ONE).ok_or(ERR_OVERFLOW)
    }

    pub fn total_debt(&self) -> Result<u128> {
        self.debt_of(self.total_scaled)
    }

    /// What suppliers collectively own: cash plus debt, less protocol reserves.
    pub fn supply_value(&self) -> Result<u128> {
        self.cash
            .checked_add(self.total_debt()?)
            .and_then(|total| total.checked_sub(self.reserves))
            .ok_or(ERR_STATE)
    }

    /// Cash the market may lend out or hand back to suppliers; the part that
    /// backs the reserves stays put.
    pub fn available(&self) -> u128 {
        self.cash.saturating_sub(self.reserves)
    }

    /// What must physically sit in the market for one currency. Collateral is
    /// the collateral asset; for the borrowed asset it is cash plus idle balances.
    /// In-flight withdrawals are excluded: their value has already left.
    pub fn held(&self, side: usize) -> Result<u128> {
        let mut total = if side == 0 { self.total_collateral } else { self.cash };
        for account in &self.accounts {
            total = total.checked_add(account.balance[side]).ok_or(ERR_OVERFLOW)?;
        }
        Ok(total)
    }

    pub fn receipt_seen(&self, hash: &[u8; 32]) -> bool {
        self.receipts.contains(hash)
    }

    pub fn push_receipt(&mut self, hash: [u8; 32]) {
        let slot = (self.receipt_pos % RECEIPT_SLOTS as u64) as usize;
        self.receipts[slot] = hash;
        self.receipt_pos = self.receipt_pos.saturating_add(1);
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != STATE_BYTES {
            return Err(ERR_STATE);
        }
        let config = Config::parse(&bytes[..CONFIG_BYTES]).ok_or(ERR_STATE)?;
        if bytes[OFF_PAUSED] > 1
            || bytes[OFF_PAUSED + 1..OFF_PRICE] != [0; 7]
            || bytes[OFF_PRICE_TS + 4..OFF_CASH] != [0; 4]
            || bytes[OFF_ACCRUAL + 4..OFF_COLLATERAL] != [0; 4]
        {
            return Err(ERR_STATE);
        }
        let mut receipts = [[0; 32]; RECEIPT_SLOTS];
        for (index, receipt) in receipts.iter_mut().enumerate() {
            *receipt = array(bytes, OFF_RECEIPTS + index * 32)?;
        }
        let mut accounts = Vec::new();
        let mut closed = false;
        for index in 0..ACCOUNT_SLOTS {
            let base = OFF_ACCOUNTS + index * ACCOUNT_BYTES;
            let account = Account {
                key: array(bytes, base)?,
                balance: [read_u128(bytes, base + 32)?, read_u128(bytes, base + 48)?],
                collateral: read_u128(bytes, base + 64)?,
                shares: read_u128(bytes, base + 80)?,
                scaled: read_u128(bytes, base + 96)?,
            };
            if account.key == [0; 32] {
                if !account.is_empty() {
                    return Err(ERR_STATE);
                }
                closed = true;
            } else if closed {
                return Err(ERR_STATE);
            } else {
                accounts.push(account);
            }
        }
        let mut pending = Vec::new();
        let mut closed = false;
        for index in 0..PENDING_SLOTS {
            let base = OFF_PENDING + index * PENDING_BYTES;
            let used = bytes[base];
            if used > 1
                || bytes[base + 1..base + 8] != [0; 7]
                || bytes[base + 108..base + 112] != [0; 4]
            {
                return Err(ERR_STATE);
            }
            let entry = Pending {
                recipient: AccountAddress {
                    dapp_id: array(bytes, base + 8)?,
                    account_id: array(bytes, base + 40)?,
                },
                holder: array(bytes, base + 72)?,
                currency: read_u32(bytes, base + 104)?,
                amount: read_u128(bytes, base + 112)?,
                id: read_u64(bytes, base + 128)?,
            };
            if used == 0 {
                let empty = entry.recipient
                    == AccountAddress { dapp_id: [0; 32], account_id: [0; 32] }
                    && entry.holder == [0; 32]
                    && entry.currency == 0
                    && entry.amount == 0
                    && entry.id == 0;
                if !empty {
                    return Err(ERR_STATE);
                }
                closed = true;
            } else if closed {
                return Err(ERR_STATE);
            } else {
                pending.push(entry);
            }
        }
        let state = Self {
            config,
            paused: bytes[OFF_PAUSED] == 1,
            price: read_u128(bytes, OFF_PRICE)?,
            price_ts: read_u32(bytes, OFF_PRICE_TS)?,
            cash: read_u128(bytes, OFF_CASH)?,
            reserves: read_u128(bytes, OFF_RESERVES)?,
            total_shares: read_u128(bytes, OFF_SHARES)?,
            total_scaled: read_u128(bytes, OFF_SCALED)?,
            index: read_u128(bytes, OFF_INDEX)?,
            last_accrual: read_u32(bytes, OFF_ACCRUAL)?,
            total_collateral: read_u128(bytes, OFF_COLLATERAL)?,
            ops: read_u64(bytes, OFF_OPS)?,
            withdraw_seq: read_u64(bytes, OFF_SEQ)?,
            receipt_pos: read_u64(bytes, OFF_RECEIPT_POS)?,
            receipts,
            accounts,
            pending,
        };
        state.validate()?;
        Ok(state)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = vec![0; STATE_BYTES];
        self.config.write_to(&mut bytes);
        bytes[OFF_PAUSED] = u8::from(self.paused);
        put(&mut bytes, OFF_PRICE, &self.price.to_le_bytes());
        put(&mut bytes, OFF_PRICE_TS, &self.price_ts.to_le_bytes());
        put(&mut bytes, OFF_CASH, &self.cash.to_le_bytes());
        put(&mut bytes, OFF_RESERVES, &self.reserves.to_le_bytes());
        put(&mut bytes, OFF_SHARES, &self.total_shares.to_le_bytes());
        put(&mut bytes, OFF_SCALED, &self.total_scaled.to_le_bytes());
        put(&mut bytes, OFF_INDEX, &self.index.to_le_bytes());
        put(&mut bytes, OFF_ACCRUAL, &self.last_accrual.to_le_bytes());
        put(&mut bytes, OFF_COLLATERAL, &self.total_collateral.to_le_bytes());
        put(&mut bytes, OFF_OPS, &self.ops.to_le_bytes());
        put(&mut bytes, OFF_SEQ, &self.withdraw_seq.to_le_bytes());
        put(&mut bytes, OFF_RECEIPT_POS, &self.receipt_pos.to_le_bytes());
        for (index, receipt) in self.receipts.iter().enumerate() {
            put(&mut bytes, OFF_RECEIPTS + index * 32, receipt);
        }
        for (index, account) in self.accounts.iter().enumerate().take(ACCOUNT_SLOTS) {
            let base = OFF_ACCOUNTS + index * ACCOUNT_BYTES;
            put(&mut bytes, base, &account.key);
            put(&mut bytes, base + 32, &account.balance[0].to_le_bytes());
            put(&mut bytes, base + 48, &account.balance[1].to_le_bytes());
            put(&mut bytes, base + 64, &account.collateral.to_le_bytes());
            put(&mut bytes, base + 80, &account.shares.to_le_bytes());
            put(&mut bytes, base + 96, &account.scaled.to_le_bytes());
        }
        for (index, entry) in self.pending.iter().enumerate().take(PENDING_SLOTS) {
            let base = OFF_PENDING + index * PENDING_BYTES;
            bytes[base] = 1;
            put(&mut bytes, base + 8, &entry.recipient.dapp_id);
            put(&mut bytes, base + 40, &entry.recipient.account_id);
            put(&mut bytes, base + 72, &entry.holder);
            put(&mut bytes, base + 104, &entry.currency.to_le_bytes());
            put(&mut bytes, base + 112, &entry.amount.to_le_bytes());
            put(&mut bytes, base + 128, &entry.id.to_le_bytes());
        }
        bytes
    }

    /// Every structural and accounting rule the contract relies on. It runs on
    /// each decode and before each state output, so a bug cannot persist a
    /// state that a later call would not accept.
    pub fn validate(&self) -> Result<()> {
        if !self.config.is_valid()
            || self.accounts.len() > ACCOUNT_SLOTS
            || self.pending.len() > PENDING_SLOTS
            || self.index < ONE
            || (self.price == 0) != (self.price_ts == 0)
        {
            return Err(ERR_STATE);
        }
        let (mut shares, mut scaled, mut collateral) = (0u128, 0u128, 0u128);
        for (index, account) in self.accounts.iter().enumerate() {
            let referenced = self.pending.iter().any(|entry| entry.holder == account.key);
            if account.key == [0; 32]
                || self.accounts[..index].iter().any(|other| other.key == account.key)
                || (account.is_empty() && !referenced)
            {
                return Err(ERR_STATE); // empty accounts are compacted away
            }
            shares = shares.checked_add(account.shares).ok_or(ERR_STATE)?;
            scaled = scaled.checked_add(account.scaled).ok_or(ERR_STATE)?;
            collateral = collateral.checked_add(account.collateral).ok_or(ERR_STATE)?;
        }
        if shares != self.total_shares
            || scaled != self.total_scaled
            || collateral != self.total_collateral
        {
            return Err(ERR_STATE);
        }
        for (index, entry) in self.pending.iter().enumerate() {
            if !nonzero(entry.recipient)
                || entry.amount == 0
                || self.side_of(entry.currency).is_none()
                || entry.id >= self.withdraw_seq
                || self.account_index(entry.holder).is_none()
                || self.pending[..index]
                    .iter()
                    .any(|other| other.recipient == entry.recipient || other.id == entry.id)
            {
                return Err(ERR_STATE);
            }
        }
        // The receipt ring holds nonzero, distinct hashes in exactly the slots
        // that have been written.
        for (index, receipt) in self.receipts.iter().enumerate() {
            let written = (index as u64) < self.receipt_pos;
            if written == (*receipt == [0; 32])
                || (written && self.receipts[..index].contains(receipt))
            {
                return Err(ERR_STATE);
            }
        }
        self.supply_value()?;
        self.held(0)?;
        self.held(1)?;
        Ok(())
    }
}
