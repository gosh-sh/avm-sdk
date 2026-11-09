//! Fixed-layout state, canonical decoding and the accounting invariants.

use avm_contract_sdk::AccountAddress;

use crate::{
    Result, ERR_CAPACITY, ERR_OVERFLOW, ERR_STATE, MAX_PROTOCOL_FEE_BPS, MAX_SWAP_FEE_BPS,
    MINIMUM_LIQUIDITY, SHELL_CURRENCY_ID,
};

pub const CONFIG_BYTES: usize = 64;
pub const ACCOUNT_SLOTS: usize = 32;
pub const PENDING_SLOTS: usize = 8;
pub const RECEIPT_SLOTS: usize = 16;

const ACCOUNT_BYTES: usize = 80;
const PENDING_BYTES: usize = 136;

const OFF_FLAGS: usize = 64;
const OFF_RESERVES: usize = 72;
const OFF_FEES: usize = 104;
const OFF_LP_TOTAL: usize = 136;
/// 40 bytes that held the TWAP accumulators in the first draft; they must be zero.
const OFF_RESERVED: usize = 152;
const OFF_OPS: usize = 192;
const OFF_SEQ: usize = 200;
const OFF_RECEIPT_POS: usize = 208;
const OFF_RECEIPTS: usize = 216;
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

/// Pool parameters. `admin`, the currencies and `fuel_reserve` are immutable;
/// the two fee fields can later be changed by the admin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    pub admin: [u8; 32],
    /// Pool assets in ascending currency-ID order.
    pub currency: [u32; 2],
    pub swap_fee_bps: u16,
    /// Share of each swap fee, in basis points, that accrues to the protocol.
    pub protocol_fee_bps: u16,
    /// SHELL reserved as FUEL for each payout message.
    pub fuel_reserve: u128,
}

impl Config {
    /// Strictly parses and validates the 64-byte wire form.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != CONFIG_BYTES {
            return None;
        }
        let config = Self {
            admin: array(bytes, 0).ok()?,
            currency: [read_u32(bytes, 32).ok()?, read_u32(bytes, 36).ok()?],
            swap_fee_bps: read_u16(bytes, 40).ok()?,
            protocol_fee_bps: read_u16(bytes, 42).ok()?,
            fuel_reserve: read_u128(bytes, 48).ok()?,
        };
        (bytes[44..48] == [0; 4] && config.is_valid()).then_some(config)
    }

    pub fn is_valid(&self) -> bool {
        self.admin != [0; 32]
            && self.currency[0] < self.currency[1]
            && !self.currency.contains(&SHELL_CURRENCY_ID)
            && self.swap_fee_bps <= MAX_SWAP_FEE_BPS
            && self.protocol_fee_bps <= MAX_PROTOCOL_FEE_BPS
            && self.fuel_reserve != 0
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = vec![0; CONFIG_BYTES];
        put(&mut bytes, 0, &self.admin);
        put(&mut bytes, 32, &self.currency[0].to_le_bytes());
        put(&mut bytes, 36, &self.currency[1].to_le_bytes());
        put(&mut bytes, 40, &self.swap_fee_bps.to_le_bytes());
        put(&mut bytes, 42, &self.protocol_fee_bps.to_le_bytes());
        put(&mut bytes, 48, &self.fuel_reserve.to_le_bytes());
        bytes
    }
}

/// A holder's internal balances. Funds here are claimable at any time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Account {
    pub key: [u8; 32],
    pub balance: [u128; 2],
    pub lp: u128,
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
    pub reserve: [u128; 2],
    /// Protocol fees accrued in the vault, claimable by the admin.
    pub fees: [u128; 2],
    /// Total LP shares, including the permanently locked minimum.
    pub lp_total: u128,
    pub ops: u64,
    pub withdraw_seq: u64,
    pub receipt_pos: u64,
    pub receipts: [[u8; 32]; RECEIPT_SLOTS],
    pub accounts: Vec<Account>,
    pub pending: Vec<Pending>,
}

impl State {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            paused: false,
            reserve: [0; 2],
            fees: [0; 2],
            lp_total: 0,
            ops: 0,
            withdraw_seq: 0,
            receipt_pos: 0,
            receipts: [[0; 32]; RECEIPT_SLOTS],
            accounts: Vec::new(),
            pending: Vec::new(),
        }
    }

    pub fn live(&self) -> bool {
        self.lp_total != 0
    }

    pub fn locked_liquidity(&self) -> u128 {
        if self.live() {
            MINIMUM_LIQUIDITY
        } else {
            0
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
                self.accounts.push(Account { key, balance: [0; 2], lp: 0 });
                self.accounts.len() - 1
            }
        };
        Ok(&mut self.accounts[index])
    }

    /// Drops empty accounts that no pending withdrawal can still refund into.
    pub fn compact(&mut self) {
        // A manual loop: `Vec::retain` instantiates two generic helpers that cost
        // more guest bytes than this does.
        let mut index = 0;
        while index < self.accounts.len() {
            let account = &self.accounts[index];
            if account.balance != [0; 2]
                || account.lp != 0
                || self.pending.iter().any(|entry| entry.holder == account.key)
            {
                index += 1;
            } else {
                self.accounts.remove(index);
            }
        }
    }

    /// What must physically sit in the vault for one pool currency: the
    /// reserve, protocol fees and holder balances. In-flight withdrawals are
    /// excluded because their value has already left in the payout message.
    pub fn held(&self, side: usize) -> Result<u128> {
        let mut total = self.reserve[side].checked_add(self.fees[side]).ok_or(ERR_OVERFLOW)?;
        for account in &self.accounts {
            total = total.checked_add(account.balance[side]).ok_or(ERR_OVERFLOW)?;
        }
        Ok(total)
    }

    /// Value in unresolved withdrawals of one pool currency.
    pub fn in_flight(&self, side: usize) -> Result<u128> {
        let mut total = 0u128;
        for entry in &self.pending {
            if entry.currency == self.config.currency[side] {
                total = total.checked_add(entry.amount).ok_or(ERR_OVERFLOW)?;
            }
        }
        Ok(total)
    }

    /// Everything the pool is answerable for in one currency: [`Self::held`]
    /// plus [`Self::in_flight`]. A settled payout leaves it; a bounced one
    /// returns to `held`.
    pub fn liability(&self, side: usize) -> Result<u128> {
        self.held(side)?.checked_add(self.in_flight(side)?).ok_or(ERR_OVERFLOW)
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
        let flags = bytes[OFF_FLAGS];
        if flags > 1 || bytes[OFF_FLAGS + 1..OFF_RESERVES] != [0; 7] {
            return Err(ERR_STATE);
        }
        if bytes[OFF_RESERVED..OFF_OPS] != [0; 40] {
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
            let key: [u8; 32] = array(bytes, base)?;
            let account = Account {
                key,
                balance: [read_u128(bytes, base + 32)?, read_u128(bytes, base + 48)?],
                lp: read_u128(bytes, base + 64)?,
            };
            if key == [0; 32] {
                if account.balance != [0; 2] || account.lp != 0 {
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
            paused: flags == 1,
            reserve: [read_u128(bytes, OFF_RESERVES)?, read_u128(bytes, OFF_RESERVES + 16)?],
            fees: [read_u128(bytes, OFF_FEES)?, read_u128(bytes, OFF_FEES + 16)?],
            lp_total: read_u128(bytes, OFF_LP_TOTAL)?,
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
        put(&mut bytes, 0, &self.config.encode());
        bytes[OFF_FLAGS] = u8::from(self.paused);
        for side in 0..2 {
            put(&mut bytes, OFF_RESERVES + side * 16, &self.reserve[side].to_le_bytes());
            put(&mut bytes, OFF_FEES + side * 16, &self.fees[side].to_le_bytes());
        }
        put(&mut bytes, OFF_LP_TOTAL, &self.lp_total.to_le_bytes());
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
            put(&mut bytes, base + 64, &account.lp.to_le_bytes());
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
        {
            return Err(ERR_STATE);
        }
        // Liquidity: an empty pool is entirely zero; a live pool keeps both
        // reserves nonzero and the locked minimum outstanding forever.
        if self.live() {
            if self.lp_total < MINIMUM_LIQUIDITY || self.reserve.contains(&0) {
                return Err(ERR_STATE);
            }
        } else if self.reserve != [0; 2] || self.fees != [0; 2] {
            return Err(ERR_STATE);
        }
        let mut shares = self.locked_liquidity();
        for (index, account) in self.accounts.iter().enumerate() {
            if account.key == [0; 32]
                || self.accounts[..index].iter().any(|other| other.key == account.key)
            {
                return Err(ERR_STATE);
            }
            let referenced = self.pending.iter().any(|entry| entry.holder == account.key);
            if account.balance == [0; 2] && account.lp == 0 && !referenced {
                return Err(ERR_STATE); // non-canonical: empty accounts are compacted away
            }
            shares = shares.checked_add(account.lp).ok_or(ERR_STATE)?;
        }
        if shares != self.lp_total {
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
        for side in 0..2 {
            self.liability(side)?;
        }
        Ok(())
    }
}
