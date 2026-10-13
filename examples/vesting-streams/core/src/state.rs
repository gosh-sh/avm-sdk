use crate::*;
use avm_contract_sdk::AccountAddress;

pub const CONFIG_BYTES: usize = 1656;
pub const STATE_BYTES: usize = 3272;
pub const ROW_BYTES: usize = 200;
pub const STATE_BASE: usize = 1672;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stream {
    pub key: [u8; 32],
    pub payout: AccountAddress,
    pub refund: AccountAddress,
    pub total: u128,
    pub start: u32,
    pub cliff: u32,
    pub end: u32,
    pub step: u32,
    pub revocable: bool,
}
impl Default for Stream {
    fn default() -> Self {
        Self {
            key: [0; 32],
            payout: zero_address(),
            refund: zero_address(),
            total: 0,
            start: 0,
            cliff: 0,
            end: 0,
            step: 0,
            revocable: false,
        }
    }
}
impl Stream {
    pub fn earned(self, now: u32) -> ResultCode<u128> {
        vested(self.total, self.start, self.cliff, self.end, self.step, now)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub owner: [u8; 32],
    pub fuel: u128,
    pub count: u8,
    pub streams: [Stream; MAX_STREAMS],
}
impl Config {
    pub fn validate(&self) -> ResultCode<()> {
        if self.owner == [0; 32]
            || self.fuel == 0
            || self.count == 0
            || usize::from(self.count) > MAX_STREAMS
        {
            return Err(E_ARGS);
        }
        let n = usize::from(self.count);
        for (i, s) in self.streams[..n].iter().enumerate() {
            if s.key == [0; 32]
                || !nonzero_address(s.payout)
                || !nonzero_address(s.refund)
                || s.payout == s.refund
            {
                return Err(E_ARGS);
            }
            s.earned(s.start)?;
            for prior in &self.streams[..i] {
                if [prior.payout, prior.refund].iter().any(|a| *a == s.payout || *a == s.refund) {
                    return Err(E_ARGS);
                }
            }
        }
        if self.streams[n..].iter().any(|s| *s != Stream::default()) {
            return Err(E_ARGS);
        }
        self.total()?;
        Ok(())
    }
    pub fn total(&self) -> ResultCode<u128> {
        self.streams[..usize::from(self.count)]
            .iter()
            .try_fold(0u128, |v, s| v.checked_add(s.total).ok_or(E_OVERFLOW))
    }
    pub fn first_start(&self) -> u32 {
        self.streams[..usize::from(self.count)].iter().map(|s| s.start).min().unwrap_or(0)
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![0; CONFIG_BYTES];
        b[..32].copy_from_slice(&self.owner);
        b[32..48].copy_from_slice(&self.fuel.to_le_bytes());
        b[48] = self.count;
        for (i, s) in self.streams.iter().enumerate() {
            let o = 56 + i * ROW_BYTES;
            b[o..o + 32].copy_from_slice(&s.key);
            put_address(&mut b, o + 32, s.payout);
            put_address(&mut b, o + 96, s.refund);
            b[o + 160..o + 176].copy_from_slice(&s.total.to_le_bytes());
            for (off, v) in [(176, s.start), (180, s.cliff), (184, s.end), (188, s.step)] {
                b[o + off..o + off + 4].copy_from_slice(&v.to_le_bytes())
            }
            b[o + 192] = u8::from(s.revocable);
        }
        b
    }
    pub fn decode(b: &[u8]) -> ResultCode<Self> {
        if b.len() != CONFIG_BYTES || b[49..56] != [0; 7] {
            return Err(E_ARGS);
        }
        let mut c = Self {
            owner: array(b, 0)?,
            fuel: u128_at(b, 32)?,
            count: b[48],
            streams: [Stream::default(); MAX_STREAMS],
        };
        for (i, s) in c.streams.iter_mut().enumerate() {
            let o = 56 + i * ROW_BYTES;
            if b[o + 192] > 1 || b[o + 193..o + 200] != [0; 7] {
                return Err(E_ARGS);
            }
            *s = Stream {
                key: array(b, o)?,
                payout: address_at(b, o + 32)?,
                refund: address_at(b, o + 96)?,
                total: u128_at(b, o + 160)?,
                start: u32_at(b, o + 176)?,
                cliff: u32_at(b, o + 180)?,
                end: u32_at(b, o + 184)?,
                step: u32_at(b, o + 188)?,
                revocable: b[o + 192] == 1,
            };
        }
        c.validate()?;
        Ok(c)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lane {
    pub pending: u128,
    pub nonce: u64,
    pub after_lt: u64,
    pub last_lt: u64,
    pub last_hash: [u8; 32],
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub cancelled: bool,
    pub cutoff: u32,
    pub cap: u128,
    pub claimed: u128,
    pub refunded: u128,
    pub lanes: [Lane; 2],
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub config: Config,
    pub funded: bool,
    pub last_time: u32,
    pub nonce: u64,
    pub progress: [Progress; MAX_STREAMS],
}
impl State {
    pub fn new(config: Config, now: u32) -> Self {
        Self {
            config,
            funded: false,
            last_time: now,
            nonce: 0,
            progress: [Progress::default(); MAX_STREAMS],
        }
    }
    pub fn cap(&self, i: usize, now: u32) -> ResultCode<u128> {
        if self.progress[i].cancelled {
            Ok(self.progress[i].cap)
        } else {
            self.config.streams[i].earned(now.max(self.last_time))
        }
    }
    pub fn due(&self, i: usize, kind: usize, now: u32) -> ResultCode<u128> {
        if !self.funded {
            return Ok(0);
        }
        let p = self.progress[i];
        let cap = self.cap(i, now)?;
        let (limit, paid) = if kind == 0 {
            (cap, p.claimed)
        } else {
            if !p.cancelled {
                return Err(E_PHASE);
            }
            (self.config.streams[i].total.checked_sub(cap).ok_or(E_STATE)?, p.refunded)
        };
        limit.checked_sub(paid).and_then(|v| v.checked_sub(p.lanes[kind].pending)).ok_or(E_STATE)
    }
    pub fn totals(&self) -> ResultCode<(u128, u128, u128)> {
        self.progress[..usize::from(self.config.count)].iter().try_fold(
            (0u128, 0u128, 0u128),
            |(a, b, c), p| {
                Ok((
                    a.checked_add(p.claimed).ok_or(E_STATE)?,
                    b.checked_add(p.refunded).ok_or(E_STATE)?,
                    c.checked_add(p.lanes[0].pending)
                        .and_then(|v| v.checked_add(p.lanes[1].pending))
                        .ok_or(E_STATE)?,
                ))
            },
        )
    }
    pub fn held(&self) -> ResultCode<u128> {
        if !self.funded {
            return Ok(0);
        }
        let (a, b, c) = self.totals()?;
        self.config
            .total()?
            .checked_sub(a)
            .and_then(|v| v.checked_sub(b))
            .and_then(|v| v.checked_sub(c))
            .ok_or(E_STATE)
    }
    pub fn validate(&self) -> ResultCode<()> {
        self.config.validate().map_err(|_| E_STATE)?;
        if !self.funded
            && (self.nonce != 0 || self.progress.iter().any(|p| *p != Progress::default()))
        {
            return Err(E_STATE);
        }
        let n = usize::from(self.config.count);
        for (i, p) in self.progress[..n].iter().enumerate() {
            let cfg = self.config.streams[i];
            if p.cancelled {
                if !cfg.revocable
                    || p.cutoff > self.last_time
                    || p.cutoff >= cfg.end
                    || p.cap != cfg.earned(p.cutoff).map_err(|_| E_STATE)?
                {
                    return Err(E_STATE);
                }
            } else if p.cutoff != 0
                || p.cap != 0
                || p.refunded != 0
                || p.lanes[1] != Lane::default()
            {
                return Err(E_STATE);
            }
            for lane in p.lanes {
                if lane.last_lt == 0 && lane.last_hash != [0; 32]
                    || lane.last_lt > 0 && lane.last_hash == [0; 32]
                {
                    return Err(E_STATE);
                }
                if lane.pending == 0 {
                    if lane.nonce != 0 || lane.after_lt != 0 {
                        return Err(E_STATE);
                    }
                } else if lane.nonce == 0 || lane.nonce > self.nonce || lane.after_lt < lane.last_lt
                {
                    return Err(E_STATE);
                }
            }
            if self.funded {
                self.due(i, 0, self.last_time)?;
                if p.cancelled {
                    self.due(i, 1, self.last_time)?;
                }
            }
        }
        if self.progress[n..].iter().any(|p| *p != Progress::default()) {
            return Err(E_STATE);
        }
        self.held()?;
        Ok(())
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut b = self.config.encode();
        b.resize(STATE_BYTES, 0);
        b[CONFIG_BYTES] = u8::from(self.funded);
        b[1660..1664].copy_from_slice(&self.last_time.to_le_bytes());
        b[1664..1672].copy_from_slice(&self.nonce.to_le_bytes());
        for (i, p) in self.progress.iter().enumerate() {
            let o = STATE_BASE + i * ROW_BYTES;
            b[o] = u8::from(p.cancelled);
            b[o + 4..o + 8].copy_from_slice(&p.cutoff.to_le_bytes());
            for (off, v) in [(8, p.cap), (24, p.claimed), (40, p.refunded)] {
                b[o + off..o + off + 16].copy_from_slice(&v.to_le_bytes())
            }
            for (kind, l) in p.lanes.iter().enumerate() {
                let a = o + 56 + kind * 72;
                b[a..a + 16].copy_from_slice(&l.pending.to_le_bytes());
                for (off, v) in [(16, l.nonce), (24, l.after_lt), (32, l.last_lt)] {
                    b[a + off..a + off + 8].copy_from_slice(&v.to_le_bytes())
                }
                b[a + 40..a + 72].copy_from_slice(&l.last_hash);
            }
        }
        b
    }
    pub fn decode(b: &[u8]) -> ResultCode<Self> {
        if b.len() != STATE_BYTES || b[1656] > 1 || b[1657..1660] != [0; 3] {
            return Err(E_STATE);
        }
        let mut s =
            Self::new(Config::decode(&b[..CONFIG_BYTES]).map_err(|_| E_STATE)?, u32_at(b, 1660)?);
        s.funded = b[1656] == 1;
        s.nonce = u64_at(b, 1664)?;
        for (i, p) in s.progress.iter_mut().enumerate() {
            let o = STATE_BASE + i * ROW_BYTES;
            if b[o] > 1 || b[o + 1..o + 4] != [0; 3] {
                return Err(E_STATE);
            }
            *p = Progress {
                cancelled: b[o] == 1,
                cutoff: u32_at(b, o + 4)?,
                cap: u128_at(b, o + 8)?,
                claimed: u128_at(b, o + 24)?,
                refunded: u128_at(b, o + 40)?,
                lanes: [Lane::default(); 2],
            };
            for (kind, l) in p.lanes.iter_mut().enumerate() {
                let a = o + 56 + kind * 72;
                *l = Lane {
                    pending: u128_at(b, a)?,
                    nonce: u64_at(b, a + 16)?,
                    after_lt: u64_at(b, a + 24)?,
                    last_lt: u64_at(b, a + 32)?,
                    last_hash: array(b, a + 40)?,
                };
            }
        }
        s.validate()?;
        Ok(s)
    }
}
