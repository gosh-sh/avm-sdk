#![allow(dead_code)]
use avm_contract_sdk::{
    decode_message_input, AccountAddress, Contract, MessageInput, MessageOutput, StateOutputMode,
    StateUpdate,
};
use avm_revenue_core::{vault::*, *};
use avm_revenue_vault::RevenueVault;

pub const SIGNERS: [[u8; 32]; 3] = [[0x51; 32], [0x52; 32], [0x53; 32]];
pub const MEMBER_KEYS: [[u8; 32]; 3] = [[0x61; 32], [0x62; 32], [0x63; 32]];
pub const STRANGER: [u8; 32] = [0x71; 32];
pub fn address(id: u8) -> AccountAddress {
    AccountAddress { dapp_id: [0x11; 32], account_id: [id; 32] }
}
pub fn config() -> Config {
    let mut members = [Member::default(); MAX_MEMBERS];
    for i in 0..3 {
        members[i] = Member {
            key: MEMBER_KEYS[i],
            destination: address(2 + i as u8),
            weight: [5000, 3000, 2000][i],
        };
    }
    Config { signers: SIGNERS, delay: 10, vote_window: 100, initial_fuel: 10000, count: 3, members }
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
            actor: SIGNERS[0],
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
pub fn fails(out: &MessageOutput, code: u32) {
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

pub struct Vault {
    pub state: Vec<u8>,
    pub time: u32,
    pub lt: u64,
}
impl Vault {
    pub fn new() -> Self {
        let s = Spec { deploy: true, body: envelope(&config().encode()), ..Spec::default() };
        let state = initialize::<RevenueVault>(&s);
        Self { state, time: 10, lt: 10 }
    }
    pub fn snapshot(&self) -> State {
        State::decode(&self.state).unwrap()
    }
    pub fn call(
        &mut self,
        actor: [u8; 32],
        method: u64,
        body: Vec<u8>,
        value: u128,
    ) -> MessageOutput {
        self.lt += 10;
        self.time += 1;
        let s = Spec {
            actor,
            method,
            body,
            value,
            time: self.time,
            lt: self.lt,
            balance: self.snapshot().held().unwrap().saturating_add(value),
            ..Spec::default()
        };
        let out = invoke::<RevenueVault>(&self.state, &s);
        self.accept(&out);
        out
    }
    pub fn accept(&mut self, out: &MessageOutput) {
        if out.status_code == 0 {
            self.state = apply(&self.state, out.state_update.as_ref().unwrap());
            self.snapshot().validate().unwrap();
        }
    }
    pub fn deposit(&mut self, value: u128) {
        assert_eq!(self.call(STRANGER, ids::DEPOSIT, vec![], value).status_code, 0);
    }
    pub fn claim(&mut self, i: usize) -> MessageOutput {
        self.call(MEMBER_KEYS[i], ids::CLAIM, vec![i as u8], 0)
    }
    pub fn receipt_spec(&mut self, index: usize, success: bool) -> Spec {
        let state = self.snapshot();
        let m = state.members[index];
        self.lt += 10;
        let mut hash = [0x81; 32];
        hash[..8].copy_from_slice(&m.nonce.to_le_bytes());
        Spec {
            source: Some(state.config.members[index].destination),
            method: ids::CALLBACK,
            body: if success { payment_body(index as u8, m.nonce, m.pending) } else { vec![] },
            value: if success { 0 } else { m.pending },
            callback: Some((if success { 0 } else { 3102 }, hash)),
            created_lt: self.lt - 1,
            lt: self.lt,
            time: self.time,
            balance: state.held().unwrap() + if success { 0 } else { m.pending },
            ..Spec::default()
        }
    }
    pub fn settle(&mut self, index: usize, success: bool) -> MessageOutput {
        let spec = self.receipt_spec(index, success);
        let out = invoke::<RevenueVault>(&self.state, &spec);
        self.accept(&out);
        out
    }
    pub fn governance(&mut self, action: u8, value: u128) {
        let nonce = self.snapshot().proposal_nonce + 1;
        let p = Proposal { nonce, action, value, ..Proposal::default() };
        assert_eq!(self.call(SIGNERS[0], ids::PROPOSE, p.body(), 0).status_code, 0);
        assert_eq!(self.call(SIGNERS[1], ids::VOTE, p.body(), 0).status_code, 0);
        self.time = self.snapshot().proposal.ready;
        assert_eq!(
            self.call(STRANGER, ids::EXECUTE, nonce.to_le_bytes().to_vec(), 0).status_code,
            0
        );
    }
}

pub fn audit_state() -> Vec<u8> {
    use avm_revenue_audit::AuditLedger;
    let s = Spec { destination: address(9), deploy: true, body: envelope(&[3]), ..Spec::default() };
    let mut b = initialize::<AuditLedger>(&s);
    for i in 0..3 {
        let mut body = vec![i as u8];
        body.extend_from_slice(&address(2 + i as u8).dapp_id);
        body.extend_from_slice(&address(2 + i as u8).account_id);
        let s = Spec { destination: address(9), method: ids::AUDIT_BIND, body, ..Spec::default() };
        let out = invoke::<AuditLedger>(&b, &s);
        assert_eq!(out.status_code, 0);
        b = apply(&b, out.state_update.as_ref().unwrap());
    }
    b
}
pub fn audit_message(index: usize, nonce: u64, amount: u128) -> Vec<u8> {
    message(&Spec {
        source: Some(address(2 + index as u8)),
        destination: address(9),
        method: ids::AUDIT_RECORD,
        body: payment_body(index as u8, nonce, amount),
        ..Spec::default()
    })
}
pub fn batch_frame(state: &[u8], messages: &[Vec<u8>]) -> Vec<u8> {
    let mut b = Vec::from(*b"ACBI");
    b.extend_from_slice(&10u64.to_le_bytes());
    b.extend_from_slice(&10u32.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes());
    b.extend_from_slice(&[0x12; 32]);
    b.extend_from_slice(&address(9).dapp_id);
    b.extend_from_slice(&address(9).account_id);
    b.extend_from_slice(&[0x44; 32]);
    b.push(1);
    b.extend_from_slice(&[0; 7]);
    b.extend_from_slice(&0u128.to_le_bytes());
    extras(&mut b, &[(2, 1_000_000)], true);
    b.extend_from_slice(&(state.len() as u32).to_le_bytes());
    b.extend_from_slice(state);
    b.extend_from_slice(&(messages.len() as u32).to_le_bytes());
    let mut offset = 0u32;
    b.extend_from_slice(&offset.to_le_bytes());
    for msg in messages {
        offset += msg.len() as u32;
        b.extend_from_slice(&offset.to_le_bytes());
    }
    for msg in messages {
        b.extend_from_slice(msg);
    }
    b.extend_from_slice(&4u32.to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b
}
