use crate::*;

pub const CONFIG_BYTES: usize = 1792;
pub const STATE_BYTES: usize = 3560;
pub const MEMBER_STATE_BASE: usize = 1896;
pub const MEMBER_STATE_BYTES: usize = 104;
pub const PAUSE: u8 = 1;
pub const RESUME: u8 = 2;
pub const SET_FUEL: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Member {
    pub key: [u8; 32],
    pub destination: AccountAddress,
    pub weight: u16,
}
impl Default for Member {
    fn default() -> Self {
        Self { key: [0; 32], destination: zero_address(), weight: 0 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub signers: [[u8; 32]; 3],
    pub delay: u32,
    pub vote_window: u32,
    pub initial_fuel: u128,
    pub count: u8,
    pub members: [Member; MAX_MEMBERS],
}
impl Config {
    pub fn decode(b: &[u8]) -> HubResult<Self> {
        if b.len() != CONFIG_BYTES || b[121..128] != [0; 7] {
            return Err(E_ARGS);
        }
        let mut signers = [[0; 32]; 3];
        for (i, k) in signers.iter_mut().enumerate() {
            *k = array(b, i * 32)?;
        }
        let mut members = [Member::default(); MAX_MEMBERS];
        for (i, m) in members.iter_mut().enumerate() {
            let o = 128 + i * 104;
            if b[o + 98..o + 104] != [0; 6] {
                return Err(E_ARGS);
            }
            *m = Member {
                key: array(b, o)?,
                destination: address_at(b, o + 32)?,
                weight: u16::from_le_bytes(array(b, o + 96)?),
            };
        }
        let c = Self {
            signers,
            delay: u32_at(b, 96)?,
            vote_window: u32_at(b, 100)?,
            initial_fuel: u128_at(b, 104)?,
            count: b[120],
            members,
        };
        c.validate()?;
        Ok(c)
    }
    pub fn validate(&self) -> HubResult<()> {
        let n = usize::from(self.count);
        if n == 0
            || n > MAX_MEMBERS
            || self.delay == 0
            || self.vote_window == 0
            || self.initial_fuel == 0
            || self
                .signers
                .iter()
                .enumerate()
                .any(|(i, k)| *k == [0; 32] || self.signers[..i].contains(k))
        {
            return Err(E_ARGS);
        }
        let mut total = 0u32;
        for (i, m) in self.members.iter().enumerate() {
            if i < n {
                if m.key == [0; 32]
                    || !nonzero_address(m.destination)
                    || m.weight == 0
                    || self.members[..i]
                        .iter()
                        .any(|p| p.key == m.key || p.destination == m.destination)
                {
                    return Err(E_ARGS);
                }
                total = total.checked_add(u32::from(m.weight)).ok_or(E_ARGS)?;
            } else if *m != Member::default() {
                return Err(E_ARGS);
            }
        }
        if total != 10_000 {
            return Err(E_ARGS);
        }
        Ok(())
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![0; CONFIG_BYTES];
        for (i, k) in self.signers.iter().enumerate() {
            b[i * 32..i * 32 + 32].copy_from_slice(k);
        }
        b[96..100].copy_from_slice(&self.delay.to_le_bytes());
        b[100..104].copy_from_slice(&self.vote_window.to_le_bytes());
        b[104..120].copy_from_slice(&self.initial_fuel.to_le_bytes());
        b[120] = self.count;
        for (i, m) in self.members.iter().enumerate() {
            let o = 128 + i * 104;
            b[o..o + 32].copy_from_slice(&m.key);
            put_address(&mut b, o + 32, m.destination);
            b[o + 96..o + 98].copy_from_slice(&m.weight.to_le_bytes());
        }
        b
    }
    pub fn signer(&self, key: [u8; 32]) -> HubResult<usize> {
        self.signers.iter().position(|k| *k == key).ok_or(E_AUTH)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemberState {
    pub claimable: u128,
    pub paid: u128,
    pub pending: u128,
    pub nonce: u64,
    pub after_lt: u64,
    pub last_callback_lt: u64,
    pub last_callback_hash: [u8; 32],
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Proposal {
    pub nonce: u64,
    pub action: u8,
    pub votes: u8,
    pub value: u128,
    pub created: u32,
    pub ready: u32,
    pub expires: u32,
}
impl Proposal {
    pub fn body(&self) -> Vec<u8> {
        let mut b = self.nonce.to_le_bytes().to_vec();
        b.push(self.action);
        b.extend_from_slice(&self.value.to_le_bytes());
        b
    }
    pub fn decode_body(b: &[u8]) -> HubResult<(u64, u8, u128)> {
        if b.len() != 25 {
            return Err(E_ARGS);
        }
        let nonce = u64_at(b, 0)?;
        let action = b[8];
        let value = u128_at(b, 9)?;
        if nonce == 0
            || !matches!(action, PAUSE | RESUME | SET_FUEL)
            || action != SET_FUEL && value != 0
            || action == SET_FUEL && value == 0
        {
            return Err(E_GOVERNANCE);
        }
        Ok((nonce, action, value))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub config: Config,
    pub received: u128,
    pub paid: u128,
    pub claim_nonce: u64,
    pub proposal_nonce: u64,
    pub paused: bool,
    pub proposal: Proposal,
    pub members: [MemberState; MAX_MEMBERS],
    pub fuel_limit: u128,
}
// fuel_limit is mutable and kept in the config's initial_fuel wire slot in state.
// This is an operating reserve, never a beneficiary weight or address.
impl State {
    pub fn new(config: Config) -> Self {
        let fuel_limit = config.initial_fuel;
        Self {
            config,
            received: 0,
            paid: 0,
            claim_nonce: 0,
            proposal_nonce: 0,
            paused: false,
            proposal: Proposal::default(),
            members: [MemberState::default(); MAX_MEMBERS],
            fuel_limit,
        }
    }
    pub fn decode(b: &[u8]) -> HubResult<Self> {
        if b.len() != STATE_BYTES
            || b[1841..1848] != [0; 7]
            || b[1840] > 1
            || b[1858..1864] != [0; 6]
            || b[1892..1896] != [0; 4]
        {
            return Err(E_STATE);
        }
        let config = Config::decode(&b[..CONFIG_BYTES]).map_err(|_| E_STATE)?;
        let fuel_limit = config.initial_fuel;
        let mut members = [MemberState::default(); MAX_MEMBERS];
        for (i, m) in members.iter_mut().enumerate() {
            let o = MEMBER_STATE_BASE + i * MEMBER_STATE_BYTES;
            *m = MemberState {
                claimable: u128_at(b, o)?,
                paid: u128_at(b, o + 16)?,
                pending: u128_at(b, o + 32)?,
                nonce: u64_at(b, o + 48)?,
                after_lt: u64_at(b, o + 56)?,
                last_callback_lt: u64_at(b, o + 64)?,
                last_callback_hash: array(b, o + 72)?,
            };
        }
        let s = Self {
            config,
            received: u128_at(b, 1792)?,
            paid: u128_at(b, 1808)?,
            claim_nonce: u64_at(b, 1824)?,
            proposal_nonce: u64_at(b, 1832)?,
            paused: b[1840] == 1,
            proposal: Proposal {
                nonce: u64_at(b, 1848)?,
                action: b[1856],
                votes: b[1857],
                value: u128_at(b, 1864)?,
                created: u32_at(b, 1880)?,
                ready: u32_at(b, 1884)?,
                expires: u32_at(b, 1888)?,
            },
            members,
            fuel_limit,
        };
        s.validate()?;
        Ok(s)
    }
    pub fn validate(&self) -> HubResult<()> {
        self.config.validate().map_err(|_| E_STATE)?;
        if self.fuel_limit == 0 {
            return Err(E_STATE);
        }
        let mut paid = 0u128;
        let mut all = 0u128;
        for (i, m) in self.members.iter().enumerate() {
            if i >= usize::from(self.config.count) {
                if *m != MemberState::default() {
                    return Err(E_STATE);
                }
                continue;
            }
            let sum = m
                .paid
                .checked_add(m.claimable)
                .and_then(|v| v.checked_add(m.pending))
                .ok_or(E_STATE)?;
            if sum != entitlement(self.received, self.config.members[i].weight)?
                || (m.pending == 0 && (m.nonce != 0 || m.after_lt != 0))
                || (m.pending != 0
                    && (m.nonce == 0
                        || m.nonce > self.claim_nonce
                        || m.after_lt < m.last_callback_lt))
                || ((m.last_callback_lt == 0) != (m.last_callback_hash == [0; 32]))
            {
                return Err(E_STATE);
            }
            paid = paid.checked_add(m.paid).ok_or(E_STATE)?;
            all = all.checked_add(sum).ok_or(E_STATE)?;
        }
        if paid != self.paid
            || all > self.received
            || self.received - all >= u128::from(self.config.count)
        {
            return Err(E_STATE);
        }
        let p = self.proposal;
        if p.nonce == 0 {
            if p != Proposal::default() {
                return Err(E_STATE);
            }
        } else if p.nonce != self.proposal_nonce
            || p.votes == 0
            || p.votes & !7 != 0
            || Proposal::decode_body(&p.body()).is_err()
            || p.created.checked_add(self.config.delay) != Some(p.ready)
            || p.ready.checked_add(self.config.vote_window) != Some(p.expires)
        {
            return Err(E_STATE);
        }
        Ok(())
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![0; STATE_BYTES];
        let mut c = self.config.clone();
        c.initial_fuel = self.fuel_limit;
        b[..CONFIG_BYTES].copy_from_slice(&c.encode());
        b[1792..1808].copy_from_slice(&self.received.to_le_bytes());
        b[1808..1824].copy_from_slice(&self.paid.to_le_bytes());
        b[1824..1832].copy_from_slice(&self.claim_nonce.to_le_bytes());
        b[1832..1840].copy_from_slice(&self.proposal_nonce.to_le_bytes());
        b[1840] = u8::from(self.paused);
        let p = self.proposal;
        b[1848..1856].copy_from_slice(&p.nonce.to_le_bytes());
        b[1856] = p.action;
        b[1857] = p.votes;
        b[1864..1880].copy_from_slice(&p.value.to_le_bytes());
        b[1880..1884].copy_from_slice(&p.created.to_le_bytes());
        b[1884..1888].copy_from_slice(&p.ready.to_le_bytes());
        b[1888..1892].copy_from_slice(&p.expires.to_le_bytes());
        for (i, m) in self.members.iter().enumerate() {
            let o = MEMBER_STATE_BASE + i * MEMBER_STATE_BYTES;
            for (off, n) in [(0, m.claimable), (16, m.paid), (32, m.pending)] {
                b[o + off..o + off + 16].copy_from_slice(&n.to_le_bytes());
            }
            for (off, n) in [(48, m.nonce), (56, m.after_lt), (64, m.last_callback_lt)] {
                b[o + off..o + off + 8].copy_from_slice(&n.to_le_bytes());
            }
            b[o + 72..o + 104].copy_from_slice(&m.last_callback_hash);
        }
        b
    }
    pub fn pending(&self) -> HubResult<u128> {
        self.members.iter().try_fold(0u128, |v, m| v.checked_add(m.pending).ok_or(E_STATE))
    }
    pub fn held(&self) -> HubResult<u128> {
        self.received
            .checked_sub(self.paid)
            .and_then(|v| v.checked_sub(self.pending().ok()?))
            .ok_or(E_STATE)
    }
    pub fn dust(&self) -> HubResult<u128> {
        let mut sum = 0u128;
        for m in &self.config.members[..usize::from(self.config.count)] {
            sum = sum.checked_add(entitlement(self.received, m.weight)?).ok_or(E_STATE)?;
        }
        self.received.checked_sub(sum).ok_or(E_STATE)
    }
    pub fn deposit(&mut self, amount: u128) -> HubResult<()> {
        if amount == 0 {
            return Err(E_VALUE);
        }
        let next = self.received.checked_add(amount).ok_or(E_OVERFLOW)?;
        let mut members = self.members;
        for (i, m) in members[..usize::from(self.config.count)].iter_mut().enumerate() {
            let w = self.config.members[i].weight;
            let delta =
                entitlement(next, w)?.checked_sub(entitlement(self.received, w)?).ok_or(E_STATE)?;
            m.claimable = m.claimable.checked_add(delta).ok_or(E_OVERFLOW)?;
        }
        self.received = next;
        self.members = members;
        self.validate()
    }
}
