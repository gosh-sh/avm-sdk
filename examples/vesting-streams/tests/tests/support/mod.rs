#![allow(dead_code)]
use avm_contract_sdk::{
    decode_message_input, AccountAddress, Contract, MessageInput, MessageOutput, StateOutputMode,
    StateUpdate,
};
use avm_stream_core::{state::*, *};
use avm_stream_vesting::Vesting;
pub const OWNER: [u8; 32] = [0x51; 32];
pub const BENEFICIARIES: [[u8; 32]; 3] = [[0x61; 32], [0x62; 32], [0x63; 32]];
pub const STRANGER: [u8; 32] = [0x71; 32];
pub fn address(id: u8) -> AccountAddress {
    AccountAddress { dapp_id: [0x11; 32], account_id: [id; 32] }
}
pub fn config() -> Config {
    let mut streams = [Stream::default(); MAX_STREAMS];
    for i in 0..3 {
        streams[i] = Stream {
            key: BENEFICIARIES[i],
            payout: address(2 + i as u8 * 2),
            refund: address(3 + i as u8 * 2),
            total: [1000, 700, 300][i],
            start: 100,
            cliff: [130, 100, 150][i],
            end: 200,
            step: [0, 30, 0][i],
            revocable: i != 2,
        };
    }
    Config { owner: OWNER, fuel: 10000, count: 3, streams }
}
pub fn envelope(data: &[u8]) -> Vec<u8> {
    let mut b = vec![0; 288];
    b[..4].copy_from_slice(b"ACDP");
    b[268..272].copy_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(data);
    b
}
#[derive(Clone)]
pub struct Spec {
    pub actor: [u8; 32],
    pub source: Option<AccountAddress>,
    pub destination: AccountAddress,
    pub method: u64,
    pub body: Vec<u8>,
    pub value: u128,
    pub extra: Vec<(u32, u128)>,
    pub balance: u128,
    pub shells: u128,
    pub time: u32,
    pub lt: u64,
    pub created_lt: u64,
    pub callback: Option<(u32, [u8; 32])>,
    pub request: bool,
    pub deploy: bool,
    pub mode: StateOutputMode,
}
impl Default for Spec {
    fn default() -> Self {
        Self {
            actor: OWNER,
            source: None,
            destination: address(1),
            method: ids::STATUS,
            body: vec![],
            value: 0,
            extra: vec![],
            balance: 1_000_000,
            shells: 1_000_000,
            time: 10,
            lt: 10,
            created_lt: 9,
            callback: None,
            request: false,
            deploy: false,
            mode: StateOutputMode::FullReplace,
        }
    }
}
fn extras(b: &mut Vec<u8>, entries: &[(u32, u128)], count: bool) {
    if count {
        b.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    }
    for (id, n) in entries {
        b.extend_from_slice(&id.to_le_bytes());
        b.extend_from_slice(&n.to_le_bytes());
    }
}
pub fn message(s: &Spec) -> Vec<u8> {
    let mut b = vec![0; if s.source.is_some() { 192 } else { 208 }];
    b[0] = u8::from(s.source.is_none());
    b[1] = if s.source.is_some() {
        u8::from(s.callback.is_some()) | if s.request { 4 } else { 0 }
    } else {
        1 | if s.deploy { 2 } else { 0 }
    };
    b[2..4].copy_from_slice(&(s.extra.len() as u16).to_le_bytes());
    b[4..8].copy_from_slice(&(s.body.len() as u32).to_le_bytes());
    b[8..16].copy_from_slice(&s.method.to_le_bytes());
    b[16..32].copy_from_slice(&s.value.to_le_bytes());
    b[32..48].copy_from_slice(&10000u128.to_le_bytes());
    b[48..56].copy_from_slice(&s.created_lt.to_le_bytes());
    b[56..60].copy_from_slice(&s.time.to_le_bytes());
    if let Some(source) = s.source {
        put_address(&mut b, 64, source);
        put_address(&mut b, 128, s.destination);
        if let Some((status, hash)) = s.callback {
            b.extend_from_slice(&hash);
            b.extend_from_slice(&s.destination.dapp_id);
            b.extend_from_slice(&s.destination.account_id);
            b.extend_from_slice(&status.to_le_bytes());
        }
        if s.request {
            b.extend_from_slice(&ids::CALLBACK.to_le_bytes());
        }
    } else {
        b[60..64].copy_from_slice(&10000u32.to_le_bytes());
        put_address(&mut b, 64, s.destination);
        b[128..160].copy_from_slice(&s.actor);
        b[160..162].copy_from_slice(&64u16.to_le_bytes());
    }
    extras(&mut b, &s.extra, false);
    b.extend_from_slice(&s.body);
    if s.source.is_none() {
        b.extend_from_slice(&[0; 64]);
    }
    b
}
pub fn frame(state: &[u8], s: &Spec) -> Vec<u8> {
    let mut b = Vec::from(*b"ACMI");
    b.extend_from_slice(&s.lt.to_le_bytes());
    b.extend_from_slice(&s.time.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes());
    b.extend_from_slice(&[0x12; 32]);
    b.push(u8::from(s.source.is_none()));
    b.extend_from_slice(&[0; 7]);
    if let Some(a) = s.source {
        b.extend_from_slice(&a.dapp_id);
        b.extend_from_slice(&a.account_id);
        b.extend_from_slice(&[0; 32]);
    } else {
        b.extend_from_slice(&[0; 64]);
        b.extend_from_slice(&s.actor);
    }
    b.extend_from_slice(&s.destination.dapp_id);
    b.extend_from_slice(&s.destination.account_id);
    b.extend_from_slice(&[0x44; 32]);
    b.push(s.mode as u8);
    b.extend_from_slice(&[0; 7]);
    b.extend_from_slice(&s.balance.to_le_bytes());
    extras(&mut b, &[(2, s.shells)], true);
    b.extend_from_slice(&(state.len() as u32).to_le_bytes());
    b.extend_from_slice(state);
    let msg = message(s);
    b.extend_from_slice(&(msg.len() as u32).to_le_bytes());
    b.extend_from_slice(&msg);
    b.extend_from_slice(&4u32.to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b
}
pub fn input<'a>(b: &'a [u8]) -> MessageInput<'a> {
    decode_message_input(b).expect("canonical frame")
}
pub fn apply(state: &[u8], update: &StateUpdate) -> Vec<u8> {
    match update {
        StateUpdate::FullReplace(b) => b.clone(),
        StateUpdate::DataPatchList(patches) => {
            let mut b = state.to_vec();
            for p in patches {
                let o = p.offset as usize;
                assert_eq!(p.old_len as usize, p.new_bytes.len());
                b[o..o + p.new_bytes.len()].copy_from_slice(&p.new_bytes);
            }
            b
        }
    }
}
pub fn fails(out: MessageOutput, code: u32) {
    assert_eq!(out.status_code, code);
    assert!(out.state_update.is_none());
    assert!(
        out.outbound_intents.is_empty()
            && out.callback_intents.is_empty()
            && out.event_intents.is_empty()
    );
}
pub fn invoke<C: Contract>(state: &[u8], s: &Spec) -> MessageOutput {
    let b = frame(state, s);
    let out = C::process_message(input(&b));
    avm_contract_sdk::encode_message_output(&out).expect("canonical output");
    out
}
pub fn initialize<C: Contract>(s: &Spec) -> Vec<u8> {
    let b = frame(&[], s);
    let out = C::init(input(&b));
    assert_eq!(out.status_code, 0);
    apply(&[], out.state_update.as_ref().unwrap())
}

pub struct Book {
    pub state: Vec<u8>,
    pub time: u32,
    pub lt: u64,
}
impl Book {
    pub fn with_config(c: Config) -> Self {
        let s =
            Spec { actor: c.owner, deploy: true, body: envelope(&c.encode()), ..Spec::default() };
        Self { state: initialize::<Vesting>(&s), time: 10, lt: 10 }
    }
    pub fn new() -> Self {
        Self::with_config(config())
    }
    pub fn funded() -> Self {
        let mut v = Self::new();
        v.fund();
        v
    }
    pub fn snapshot(&self) -> State {
        State::decode(&self.state).unwrap()
    }
    pub fn accept(&mut self, out: &MessageOutput) {
        if out.status_code == 0 {
            self.state = apply(&self.state, out.state_update.as_ref().unwrap());
            self.snapshot().validate().unwrap()
        }
    }
    pub fn spec(
        &mut self,
        actor: [u8; 32],
        method: u64,
        body: Vec<u8>,
        value: u128,
        time: u32,
    ) -> Spec {
        self.lt += 10;
        self.time = time;
        Spec {
            actor,
            method,
            body,
            value,
            time,
            lt: self.lt,
            balance: self.snapshot().held().unwrap().saturating_add(value),
            ..Spec::default()
        }
    }
    pub fn call(
        &mut self,
        actor: [u8; 32],
        method: u64,
        body: Vec<u8>,
        value: u128,
        time: u32,
    ) -> MessageOutput {
        let s = self.spec(actor, method, body, value, time);
        let out = invoke::<Vesting>(&self.state, &s);
        self.accept(&out);
        out
    }
    pub fn fund(&mut self) {
        let total = self.snapshot().config.total().unwrap();
        assert_eq!(self.call(OWNER, ids::FUND, vec![], total, 11).status_code, 0)
    }
    pub fn claim(&mut self, i: usize, time: u32) -> MessageOutput {
        let actor = self.snapshot().config.streams[i].key;
        self.call(actor, ids::CLAIM, vec![i as u8], 0, time)
    }
    pub fn cancel(&mut self, i: usize, time: u32) -> MessageOutput {
        self.call(OWNER, ids::CANCEL, vec![i as u8], 0, time)
    }
    pub fn refund(&mut self, i: usize, time: u32) -> MessageOutput {
        self.call(OWNER, ids::REFUND, vec![i as u8], 0, time)
    }
    pub fn receipt(&mut self, i: usize, kind: usize, success: bool) -> Spec {
        let s = self.snapshot();
        let c = s.config.streams[i];
        let l = s.progress[i].lanes[kind];
        self.lt += 10;
        let mut hash = [0x81; 32];
        hash[..8].copy_from_slice(&l.nonce.to_le_bytes());
        Spec {
            source: Some(if kind == 0 { c.payout } else { c.refund }),
            method: ids::CALLBACK,
            body: if success {
                payment_body(i as u8, kind as u8, l.nonce, l.pending)
            } else {
                vec![]
            },
            value: if success { 0 } else { l.pending },
            callback: Some((if success { 0 } else { 4102 }, hash)),
            created_lt: self.lt - 1,
            lt: self.lt,
            time: self.time,
            balance: s.held().unwrap() + if success { 0 } else { l.pending },
            ..Spec::default()
        }
    }
    pub fn settle(&mut self, i: usize, kind: usize, success: bool) -> MessageOutput {
        let s = self.receipt(i, kind, success);
        let out = invoke::<Vesting>(&self.state, &s);
        self.accept(&out);
        out
    }
}
